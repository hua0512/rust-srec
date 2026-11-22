pub use media_types::Resolution;

use bytes::BytesMut;
use memchr::memchr;
use ts::{PesHeader, StreamType, TsPacketRef};

/// SPS-based resolution extraction helpers shared by
/// `StreamingResolutionDetector` and `VideoProbe`.
///
/// Extracts video resolution from H.264/H.265 SPS (Sequence Parameter Set) NAL units
/// found in MPEG-TS packets. Uses multiple detection strategies:
///
/// 1. **Simple scanning**: Scans individual TS packet payloads for complete SPS NAL units
/// 2. **PES reassembly**: Reassembles fragmented PES packets across multiple TS packets
///    to handle SPS that spans packet boundaries
///
/// # Performance
///
/// - Uses `memchr` for fast byte scanning (SIMD-accelerated)
/// - Single-pass iteration over packets where possible
/// - Early exit on first successful SPS parse
/// - Minimal allocations with capacity hints
pub(crate) struct ResolutionDetector;

pub(crate) struct StreamingResolutionDetector {
    probes: Vec<VideoProbe>,
    resolution: Option<Resolution>,
}

struct VideoProbe {
    pid: u16,
    stream_type: StreamType,
    current_pes: BytesMut,
    expected_pes_len: Option<usize>,
    pes_scan_offset: usize,
    in_pes: bool,
}

impl StreamingResolutionDetector {
    const MAX_PES_PROBE_BYTES: usize = 256 * 1024;

    pub(crate) fn new() -> Self {
        Self {
            probes: Vec::new(),
            resolution: None,
        }
    }

    pub(crate) fn add_video_stream(&mut self, pid: u16, stream_type: StreamType) {
        if !matches!(stream_type, StreamType::H264 | StreamType::H265)
            || self.probes.iter().any(|probe| probe.pid == pid)
        {
            return;
        }

        self.probes.push(VideoProbe {
            pid,
            stream_type,
            current_pes: BytesMut::with_capacity(4096),
            expected_pes_len: None,
            pes_scan_offset: 0,
            in_pes: false,
        });
    }

    pub(crate) fn push_packet(&mut self, packet: &TsPacketRef) {
        if self.resolution.is_some() {
            return;
        }

        let resolution = self
            .probes
            .iter_mut()
            .find(|probe| probe.pid == packet.pid)
            .and_then(|probe| probe.push_packet(packet));
        if resolution.is_some() {
            self.resolution = resolution;
        }
    }

    pub(crate) fn finish(mut self) -> Option<Resolution> {
        if self.resolution.is_some() {
            return self.resolution;
        }

        self.probes.iter_mut().find_map(VideoProbe::finish)
    }
}

impl VideoProbe {
    fn push_packet(&mut self, packet: &TsPacketRef) -> Option<Resolution> {
        let payload = packet.payload_slice()?;
        if let Some(resolution) =
            ResolutionDetector::scan_payload_for_sps(payload, self.stream_type)
        {
            return Some(resolution);
        }

        if packet.payload_unit_start_indicator {
            let previous_resolution = self.finish();
            self.in_pes = true;
            self.expected_pes_len = Self::expected_pes_len(payload);
            self.current_pes.clear();
            self.pes_scan_offset = 0;
            self.append_payload(payload);
            if previous_resolution.is_some() {
                return previous_resolution;
            }
        } else if self.in_pes {
            self.append_payload(payload);
        }

        if self.in_pes {
            // SPS NAL units always precede VCL slice data within an access unit
            // (ITU-T H.264 §7.4.1.2.3, H.265 §7.4.2.4.4). Once a VCL start code
            // or the probe byte cap is reached, inspect the prefix collected so
            // far and stop buffering the rest of this PES packet.
            if self.reached_vcl_slice()
                || self.current_pes.len() >= StreamingResolutionDetector::MAX_PES_PROBE_BYTES
                || self
                    .expected_pes_len
                    .is_some_and(|expected| self.current_pes.len() >= expected)
            {
                return self.finish();
            }
        }

        None
    }

    fn reached_vcl_slice(&mut self) -> bool {
        let Ok(header) = PesHeader::parse(&self.current_pes) else {
            return false;
        };
        if header.payload_offset >= self.current_pes.len() {
            return false;
        }

        let scan_from = self.pes_scan_offset.max(header.payload_offset);
        if ResolutionDetector::contains_vcl_nalu(&self.current_pes[scan_from..], self.stream_type) {
            return true;
        }
        // Retain up to 5 trailing bytes so a 4-byte start code plus 2-byte HEVC
        // NAL unit header split across TS packets is examined on the next push.
        self.pes_scan_offset = self
            .current_pes
            .len()
            .saturating_sub(5)
            .max(header.payload_offset);
        false
    }

    fn append_payload(&mut self, payload: &[u8]) {
        let remaining =
            StreamingResolutionDetector::MAX_PES_PROBE_BYTES.saturating_sub(self.current_pes.len());
        let copy_len = remaining.min(payload.len());
        self.current_pes.extend_from_slice(&payload[..copy_len]);
    }

    fn expected_pes_len(payload: &[u8]) -> Option<usize> {
        if payload.len() < 6 || payload[..3] != [0, 0, 1] {
            return None;
        }

        let packet_len = u16::from_be_bytes([payload[4], payload[5]]) as usize;
        (packet_len > 0).then_some(packet_len + 6)
    }

    fn finish(&mut self) -> Option<Resolution> {
        let resolution = (self.in_pes && self.current_pes.len() >= 9)
            .then(|| ResolutionDetector::try_parse_pes(&self.current_pes, self.stream_type))
            .flatten();
        self.current_pes.clear();
        self.expected_pes_len = None;
        self.pes_scan_offset = 0;
        self.in_pes = false;
        resolution
    }
}

impl ResolutionDetector {
    /// Scan a single TS packet payload for SPS NAL units
    #[inline]
    fn scan_payload_for_sps(payload: &[u8], stream_type: StreamType) -> Option<Resolution> {
        match stream_type {
            StreamType::H264 => Self::find_and_parse_h264_sps(payload),
            StreamType::H265 => Self::find_and_parse_h265_sps(payload),
            _ => None,
        }
    }

    /// Find and parse H.264 SPS using fast byte scanning
    ///
    /// Uses memchr to find potential start code positions, then validates.
    /// This is faster than searching for the full 3/4 byte pattern.
    fn find_and_parse_h264_sps(data: &[u8]) -> Option<Resolution> {
        let mut pos = 0;

        while pos + 4 < data.len() {
            // Fast scan for 0x00 byte (potential start of start code)
            let zero_pos = match memchr(0x00, &data[pos..]) {
                Some(p) => pos + p,
                None => break,
            };

            // Check for start code patterns: 00 00 01 or 00 00 00 01
            let (nal_start, start_code_len) =
                if zero_pos + 3 < data.len() && data[zero_pos + 1] == 0x00 {
                    if data[zero_pos + 2] == 0x01 {
                        (zero_pos + 3, 3)
                    } else if zero_pos + 4 < data.len()
                        && data[zero_pos + 2] == 0x00
                        && data[zero_pos + 3] == 0x01
                    {
                        (zero_pos + 4, 4)
                    } else {
                        pos = zero_pos + 1;
                        continue;
                    }
                } else {
                    pos = zero_pos + 1;
                    continue;
                };

            if nal_start >= data.len() {
                break;
            }

            let nal_header = data[nal_start];
            let nal_type = nal_header & 0x1F;

            // H.264 SPS NAL type is 7
            if nal_type == 7 {
                let nal_end = Self::find_nal_end_fast(&data[nal_start..])
                    .map(|end| nal_start + end)
                    .unwrap_or(data.len());

                let sps_data = &data[nal_start..nal_end];
                if let Ok(sps) =
                    h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(sps_data))
                {
                    return Some(Resolution::new(sps.width() as u32, sps.height() as u32));
                }
            }

            pos = zero_pos + start_code_len;
        }

        None
    }

    /// Find and parse H.265 SPS using fast byte scanning
    fn find_and_parse_h265_sps(data: &[u8]) -> Option<Resolution> {
        let mut pos = 0;

        while pos + 5 < data.len() {
            // Fast scan for 0x00 byte
            let zero_pos = match memchr(0x00, &data[pos..]) {
                Some(p) => pos + p,
                None => break,
            };

            // Check for start code patterns
            let (nal_start, start_code_len) =
                if zero_pos + 3 < data.len() && data[zero_pos + 1] == 0x00 {
                    if data[zero_pos + 2] == 0x01 {
                        (zero_pos + 3, 3)
                    } else if zero_pos + 4 < data.len()
                        && data[zero_pos + 2] == 0x00
                        && data[zero_pos + 3] == 0x01
                    {
                        (zero_pos + 4, 4)
                    } else {
                        pos = zero_pos + 1;
                        continue;
                    }
                } else {
                    pos = zero_pos + 1;
                    continue;
                };

            if nal_start >= data.len() {
                break;
            }

            let nal_header = data[nal_start];
            let nal_type = (nal_header & 0x7E) >> 1;

            // H.265 SPS NAL type is 33
            if nal_type == 33 {
                let nal_end = Self::find_nal_end_fast(&data[nal_start..])
                    .map(|end| nal_start + end)
                    .unwrap_or(data.len());

                let sps_data = &data[nal_start..nal_end];
                if let Ok(sps) = h265::SpsNALUnit::parse(std::io::Cursor::new(sps_data)) {
                    return Some(Resolution::new(
                        sps.rbsp.cropped_width() as u32,
                        sps.rbsp.cropped_height() as u32,
                    ));
                }
            }

            pos = zero_pos + start_code_len;
        }

        None
    }

    /// Find the end of a NAL unit using fast byte scanning
    #[inline]
    fn find_nal_end_fast(data: &[u8]) -> Option<usize> {
        let mut pos = 1; // Start from 1 to skip current NAL header

        while pos + 2 < data.len() {
            // Fast scan for 0x00
            let zero_pos = pos + memchr(0x00, &data[pos..])?;

            // Check for start code
            if zero_pos + 2 < data.len()
                && data[zero_pos + 1] == 0x00
                && (data[zero_pos + 2] == 0x01
                    || (zero_pos + 3 < data.len()
                        && data[zero_pos + 2] == 0x00
                        && data[zero_pos + 3] == 0x01))
            {
                return Some(zero_pos);
            }

            pos = zero_pos + 1;
        }

        None
    }

    /// Try to parse SPS from a PES packet
    #[inline]
    fn try_parse_pes(pes_data: &[u8], stream_type: StreamType) -> Option<Resolution> {
        Self::extract_elementary_stream_from_pes(pes_data)
            .and_then(|es| Self::scan_payload_for_sps(es, stream_type))
    }

    /// Extract elementary stream data from PES packet
    #[inline]
    fn extract_elementary_stream_from_pes(pes_data: &[u8]) -> Option<&[u8]> {
        let header = PesHeader::parse(pes_data).ok()?;
        if header.payload_offset < pes_data.len() {
            Some(&pes_data[header.payload_offset..])
        } else {
            None
        }
    }

    /// Check whether an Annex B elementary-stream slice contains any VCL slice
    /// NAL unit (`1..=5` for H.264, `0..=31` for H.265).
    fn contains_vcl_nalu(data: &[u8], stream_type: StreamType) -> bool {
        let mut pos = 0;

        while pos + 3 < data.len() {
            let Some(rel) = memchr(0x00, &data[pos..]) else {
                break;
            };
            let zero_pos = pos + rel;

            let (nal_start, start_code_len) =
                if zero_pos + 3 < data.len() && data[zero_pos + 1] == 0x00 {
                    if data[zero_pos + 2] == 0x01 {
                        (zero_pos + 3, 3)
                    } else if zero_pos + 4 < data.len()
                        && data[zero_pos + 2] == 0x00
                        && data[zero_pos + 3] == 0x01
                    {
                        (zero_pos + 4, 4)
                    } else {
                        pos = zero_pos + 1;
                        continue;
                    }
                } else {
                    pos = zero_pos + 1;
                    continue;
                };

            match stream_type {
                StreamType::H264 => {
                    let nal_header = data[nal_start];
                    if nal_header & 0x80 == 0 && (1..=5).contains(&(nal_header & 0x1F)) {
                        return true;
                    }
                }
                StreamType::H265 => {
                    if nal_start + 1 < data.len() {
                        let nal_header = data[nal_start];
                        let nuh_tid_plus1 = data[nal_start + 1] & 0x07;
                        let nal_type = (nal_header & 0x7E) >> 1;
                        if nal_header & 0x80 == 0 && nuh_tid_plus1 != 0 && nal_type <= 31 {
                            return true;
                        }
                    }
                }
                _ => return false,
            }

            pos = zero_pos + start_code_len;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sps_after_sei_payload_with_isolated_zero_bytes() {
        // AVC SPS from a 640x352 FFmpeg encode; HEVC SPS from a real OBS 1080p
        // encode (coded 1920x1088 luma samples with bottom conformance crop of 8).
        let avc = [
            0x67, 0x42, 0xc0, 0x1e, 0xda, 0x02, 0x80, 0xb6, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00,
            0x04, 0x00, 0x00, 0x03, 0x00, 0xc8, 0x3c, 0x58, 0xba, 0x80,
        ];
        let hevc = b"\x42\x01\x01\x01\x40\x00\x00\x03\x00\x90\x00\x00\x03\x00\x00\x03\x00\x78\xa0\x03\xc0\x80\x11\x07\xcb\x96\xb4\xa4\x25\x92\xe3\x01\x6a\x02\x02\x02\x08\x00\x00\x03\x00\x08\x00\x00\x03\x00\xf3\x00\x2e\xf2\x88\x00\x02\x62\x5a\x00\x00\x13\x12\xd0\x20";
        for (codec, sps, sei_header, resolution) in [
            (
                StreamType::H264,
                avc.as_slice(),
                &[6][..],
                Resolution::new(640, 352),
            ),
            (
                StreamType::H265,
                hevc.as_slice(),
                &[0x4e, 1][..],
                Resolution::new(1920, 1080),
            ),
        ] {
            for prefix in [&[0, 0, 1][..], &[0, 0, 0, 1][..]] {
                let mut payload = prefix.to_vec();
                payload.extend_from_slice(sei_header);
                // user_data_unregistered: 16-byte UUID, then rbsp_trailing_bits.
                payload.extend_from_slice(&[5, 16]);
                payload.extend((0..16).map(|value| value * 17));
                payload.push(0x80);
                // SPS carries the Annex B zero_byte before the start-code prefix.
                payload.extend_from_slice(&[0, 0, 0, 1]);
                payload.extend_from_slice(sps);
                assert_eq!(
                    ResolutionDetector::scan_payload_for_sps(&payload, codec),
                    Some(resolution)
                );
            }
        }
    }

    #[test]
    fn test_find_nal_end_fast_three_byte() {
        // NAL data followed by 3-byte start code
        let data = [0x67, 0x42, 0x00, 0x1f, 0x00, 0x00, 0x01, 0x68];
        let result = ResolutionDetector::find_nal_end_fast(&data);
        assert_eq!(result, Some(4));
    }

    #[test]
    fn test_find_nal_end_fast_four_byte() {
        // NAL data followed by 4-byte start code
        let data = [0x67, 0x42, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x01, 0x68];
        let result = ResolutionDetector::find_nal_end_fast(&data);
        assert_eq!(result, Some(4));
    }

    #[test]
    fn test_find_nal_end_fast_no_end() {
        // NAL data without following start code
        let data = [0x67, 0x42, 0x00, 0x1f, 0x96, 0x52];
        let result = ResolutionDetector::find_nal_end_fast(&data);
        assert_eq!(result, None);
    }

    #[test]
    fn truncated_h264_sps_with_three_byte_start_code_is_rejected() {
        let data = [0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f];
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        assert!(result.is_none());
    }

    #[test]
    fn truncated_h264_sps_with_four_byte_start_code_is_rejected() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f];
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_h264_sps_no_sps_present() {
        // PPS NAL type (8) instead of SPS (7)
        let data = [0x00, 0x00, 0x01, 0x68, 0x42, 0x00, 0x1f];
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        assert!(result.is_none());
    }

    #[test]
    fn capped_unbounded_pes_parses_the_retained_prefix() {
        let sps = [
            0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x78, 0x02, 0x27, 0xe5, 0xc0, 0x44, 0x00,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xca, 0x3c, 0x48, 0x96, 0x11, 0x80,
        ];
        let mut complete_sps = vec![0, 0, 1];
        complete_sps.extend_from_slice(&sps);
        let expected = ResolutionDetector::scan_payload_for_sps(&complete_sps, StreamType::H264)
            .expect("test SPS should be parseable");

        let split_at = 8;
        let remaining_sps = &sps[split_at..];
        let first_len = StreamingResolutionDetector::MAX_PES_PROBE_BYTES - remaining_sps.len();
        let mut first_payload = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x80, 0x00, 0x00];
        first_payload.resize(first_len - 3 - split_at, 0xFF);
        first_payload.extend_from_slice(&[0, 0, 1]);
        first_payload.extend_from_slice(&sps[..split_at]);
        let mut second_payload = remaining_sps.to_vec();
        second_payload.push(0xFF);

        assert_eq!(first_payload.len(), first_len);
        assert!(
            ResolutionDetector::scan_payload_for_sps(&first_payload, StreamType::H264).is_none()
        );
        assert!(
            ResolutionDetector::scan_payload_for_sps(&second_payload, StreamType::H264).is_none()
        );

        let mut probe = VideoProbe {
            pid: 256,
            stream_type: StreamType::H264,
            current_pes: BytesMut::new(),
            expected_pes_len: None,
            pes_scan_offset: 0,
            in_pes: true,
        };
        probe.append_payload(&first_payload);
        probe.append_payload(&second_payload);

        assert_eq!(
            probe.current_pes.len(),
            StreamingResolutionDetector::MAX_PES_PROBE_BYTES
        );
        assert_eq!(probe.finish(), Some(expected));
    }

    #[test]
    fn video_probe_stops_buffering_pes_once_vcl_slice_is_reached() {
        fn ts_packet(pid: u16, pusi: bool, counter: u8, payload: &[u8]) -> bytes::Bytes {
            assert!(payload.len() <= 184);
            let mut pkt = vec![0xffu8; 188];
            let pusi_bit = if pusi { 0x40 } else { 0x00 };
            let adaptation_len = 184 - payload.len();
            pkt[0] = 0x47;
            pkt[1] = pusi_bit | ((pid >> 8) as u8 & 0x1f);
            pkt[2] = pid as u8;
            if adaptation_len == 0 {
                pkt[3] = 0x10 | (counter & 0x0f);
                pkt[4..].copy_from_slice(payload);
            } else {
                pkt[3] = 0x30 | (counter & 0x0f);
                pkt[4] = (adaptation_len - 1) as u8;
                if adaptation_len > 1 {
                    pkt[5] = 0x00;
                }
                pkt[4 + adaptation_len..].copy_from_slice(payload);
            }
            bytes::Bytes::from(pkt)
        }

        let mut probe = VideoProbe {
            pid: 0x0100,
            stream_type: StreamType::H264,
            current_pes: BytesMut::with_capacity(4096),
            expected_pes_len: None,
            pes_scan_offset: 0,
            in_pes: false,
        };

        // Non-IDR PES: PES header + AUD (type 9) + non-IDR slice (type 1 = 0x41).
        let mut p_frame_start = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x80, 0x00, 0x00];
        p_frame_start.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]);
        p_frame_start.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9A, 0x24]);
        let pkt1_bytes = ts_packet(0x0100, true, 0, &p_frame_start);
        let pkt1 = TsPacketRef::parse(pkt1_bytes).unwrap();
        assert_eq!(probe.push_packet(&pkt1), None);
        assert!(!probe.in_pes);
        assert!(probe.current_pes.is_empty());

        // Subsequent slice continuation packets are not copied into current_pes.
        let pkt2_bytes = ts_packet(0x0100, false, 1, &[0xAB; 184]);
        let pkt2 = TsPacketRef::parse(pkt2_bytes).unwrap();
        assert_eq!(probe.push_packet(&pkt2), None);
        assert!(probe.current_pes.is_empty());

        // Fragmented SPS before an IDR slice (type 5 = 0x65) is still reassembled and parsed.
        let sps = [
            0x67, 0x42, 0xc0, 0x1e, 0xda, 0x02, 0x80, 0xb6, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00,
            0x04, 0x00, 0x00, 0x03, 0x00, 0xc8, 0x3c, 0x58, 0xba, 0x80,
        ];
        let mut idr_part1 = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x80, 0x00, 0x00];
        idr_part1.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        idr_part1.extend_from_slice(&sps[..10]);

        let mut idr_part2 = sps[10..].to_vec();
        idr_part2.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE, 0x3C, 0x80]);
        idr_part2.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84]);

        let idr_pkt1_bytes = ts_packet(0x0100, true, 2, &idr_part1);
        let idr_pkt1 = TsPacketRef::parse(idr_pkt1_bytes).unwrap();
        assert_eq!(probe.push_packet(&idr_pkt1), None);
        assert!(probe.in_pes);

        let idr_pkt2_bytes = ts_packet(0x0100, false, 3, &idr_part2);
        let idr_pkt2 = TsPacketRef::parse(idr_pkt2_bytes).unwrap();
        assert_eq!(
            probe.push_packet(&idr_pkt2),
            Some(Resolution::new(640, 352))
        );
        assert!(!probe.in_pes);
    }
}
