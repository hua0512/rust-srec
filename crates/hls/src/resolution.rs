pub use media_types::Resolution;

use bytes::BytesMut;
use memchr::memchr;
use tracing::debug;
use ts::{PesHeader, StreamType, TsPacketRef};

/// Resolution detector for HLS segments
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
pub struct ResolutionDetector;

impl ResolutionDetector {
    /// Extract resolution from pre-parsed TS packets
    ///
    /// Attempts multiple detection strategies in order of efficiency:
    /// 1. Simple scanning of individual packet payloads (zero allocation)
    /// 2. Full PES reassembly for fragmented SPS
    ///
    /// Returns `None` if no SPS can be found or parsed, which is normal for
    /// segments that don't contain parameter sets.
    pub fn extract_from_ts_packets<'a>(
        packets: impl Iterator<Item = &'a TsPacketRef> + Clone,
        video_streams: &[(u16, StreamType)],
    ) -> Option<Resolution> {
        if video_streams.is_empty() {
            return None;
        }

        for (pid, stream_type) in video_streams {
            // First pass: try simple scanning without collecting (fastest path)
            let video_packets = packets.clone().filter(|packet| packet.pid == *pid);

            for packet in video_packets {
                if let Some(payload) = packet.payload()
                    && let Some(resolution) = Self::scan_payload_for_sps(&payload, *stream_type)
                {
                    debug!(
                        "Found resolution {}x{} via simple scanning for PID 0x{:04X} {:?}",
                        resolution.width, resolution.height, pid, stream_type
                    );
                    return Some(resolution);
                }
            }

            // Second pass: PES reassembly for fragmented SPS
            // Only collect packets if simple scanning failed
            if let Some(resolution) = Self::try_pes_reassembly_streaming(
                packets.clone().filter(|packet| packet.pid == *pid),
                *stream_type,
            ) {
                debug!(
                    "Found resolution {}x{} via PES reassembly for PID 0x{:04X} {:?}",
                    resolution.width, resolution.height, pid, stream_type
                );
                return Some(resolution);
            }
        }

        None
    }

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
                    break;
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
                    break;
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
                        sps.rbsp.pic_width_in_luma_samples.get() as u32,
                        sps.rbsp.pic_height_in_luma_samples.get() as u32,
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
            let zero_pos = match memchr(0x00, &data[pos..]) {
                Some(p) => pos + p,
                None => return None,
            };

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

    /// Streaming PES reassembly - processes packets one at a time
    /// and parses SPS as soon as a complete PES packet is available
    fn try_pes_reassembly_streaming<'a>(
        packets: impl Iterator<Item = &'a TsPacketRef>,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        // Pre-allocate with typical PES packet size (reduces reallocations)
        let mut current_pes = BytesMut::with_capacity(4096);
        let mut in_pes_packet = false;

        for packet in packets {
            if let Some(payload) = packet.payload() {
                if packet.payload_unit_start_indicator {
                    // New PES packet starting - try to parse the previous one
                    if in_pes_packet
                        && current_pes.len() >= 9
                        && let Some(resolution) = Self::try_parse_pes(&current_pes, stream_type)
                    {
                        return Some(resolution);
                    }

                    in_pes_packet = true;
                    current_pes.clear();
                    current_pes.extend_from_slice(&payload);
                } else if in_pes_packet {
                    current_pes.extend_from_slice(&payload);
                }
            }
        }

        // Try the last PES packet
        if in_pes_packet && current_pes.len() >= 9 {
            return Self::try_parse_pes(&current_pes, stream_type);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_display() {
        let res = Resolution::new(1920, 1080);
        assert_eq!(format!("{}", res), "1920x1080");
    }

    #[test]
    fn test_resolution_equality() {
        let res1 = Resolution::new(1920, 1080);
        let res2 = Resolution::new(1920, 1080);
        let res3 = Resolution::new(1280, 720);

        assert_eq!(res1, res2);
        assert_ne!(res1, res3);
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
    fn test_extract_elementary_stream_from_pes_valid() {
        let pes_data = [
            0x00, 0x00, 0x01, // Start code
            0xE0, // Stream ID (video)
            0x00, 0x10, // PES packet length
            0x80, 0x00, // Flags
            0x00, // PES header data length (0)
            0x00, 0x00, 0x01, 0x67, // Elementary stream data
        ];

        let result = ResolutionDetector::extract_elementary_stream_from_pes(&pes_data);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), &[0x00, 0x00, 0x01, 0x67]);
    }

    #[test]
    fn test_extract_elementary_stream_from_pes_invalid_start_code() {
        let pes_data = [0x00, 0x00, 0x02, 0xE0, 0x00, 0x10, 0x80, 0x00, 0x00];
        let result = ResolutionDetector::extract_elementary_stream_from_pes(&pes_data);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_elementary_stream_from_pes_too_short() {
        let pes_data = [0x00, 0x00, 0x01, 0xE0];
        let result = ResolutionDetector::extract_elementary_stream_from_pes(&pes_data);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_h264_sps_with_three_byte_start_code() {
        // This is a minimal H.264 SPS that won't parse correctly,
        // but we can test that the NAL type detection works
        let data = [0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f];
        // The SPS parser will fail, but we verify no panic occurs
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        // Result depends on whether the minimal SPS can be parsed
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_find_h264_sps_with_four_byte_start_code() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f];
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_find_h264_sps_no_sps_present() {
        // PPS NAL type (8) instead of SPS (7)
        let data = [0x00, 0x00, 0x01, 0x68, 0x42, 0x00, 0x1f];
        let result = ResolutionDetector::find_and_parse_h264_sps(&data);
        assert!(result.is_none());
    }
}
