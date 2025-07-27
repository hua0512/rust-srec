use std::sync::OnceLock;

use bytes::{Bytes, BytesMut};
use memchr::memmem;
use tracing::{debug, warn};
use ts::{StreamType, TsPacketRef};

/// Represents video resolution information
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Resolution detector for HLS segments
pub struct ResolutionDetector;

static THREE_BYTE_FINDER: OnceLock<memmem::Finder<'static>> = OnceLock::new();
static FOUR_BYTE_FINDER: OnceLock<memmem::Finder<'static>> = OnceLock::new();

impl ResolutionDetector {
    fn three_byte_finder() -> &'static memmem::Finder<'static> {
        THREE_BYTE_FINDER.get_or_init(|| memmem::Finder::new(b"\x00\x00\x01"))
    }
    fn four_byte_finder() -> &'static memmem::Finder<'static> {
        FOUR_BYTE_FINDER.get_or_init(|| memmem::Finder::new(b"\x00\x00\x00\x01"))
    }

    /// Extract resolution from pre-parsed TS packets
    pub fn extract_from_ts_packets<'a>(
        packets: impl Iterator<Item = &'a TsPacketRef> + Clone,
        video_streams: &[(u16, StreamType)],
    ) -> Option<Resolution> {
        if video_streams.is_empty() {
            return None;
        }

        for (pid, stream_type) in video_streams {
            let video_packets = packets.clone().filter(|packet| packet.pid == *pid);

            // Try simple scanning first
            if let Some(resolution) = Self::try_simple_scanning(video_packets.clone(), *stream_type)
            {
                debug!(
                    "Found resolution {}x{} via simple scanning for PID 0x{:04X} {:?}",
                    resolution.width, resolution.height, pid, stream_type
                );
                return Some(resolution);
            }

            // Fallback to PES reassembly
            if let Some(resolution) = Self::try_pes_reassembly(video_packets, *stream_type) {
                debug!(
                    "Found resolution {}x{} via PES reassembly for PID 0x{:04X} {:?}",
                    resolution.width, resolution.height, pid, stream_type
                );
                return Some(resolution);
            }
        }

        warn!("No resolution found for video streams, using default resolution");
        // Fallback to typical resolutions based on codec
        Self::get_default_resolution(&video_streams[0].1)
    }

    /// Quick scanning of TS packet payloads for SPS (works when SPS fits in single packet)
    fn try_simple_scanning<'a>(
        video_packets: impl Iterator<Item = &'a TsPacketRef>,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        for packet in video_packets {
            if let Some(payload) = packet.payload() {
                if let Some(resolution) = Self::scan_payload_for_sps(&payload, stream_type) {
                    return Some(resolution);
                }
            }
        }
        None
    }

    /// Full PES reassembly approach (handles fragmented SPS across multiple TS packets)
    fn try_pes_reassembly<'a>(
        video_packets: impl Iterator<Item = &'a TsPacketRef>,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        let pes_data = Self::reassemble_pes_from_ts_packets(video_packets)?;
        let elementary_stream = Self::extract_elementary_stream_from_pes(&pes_data)?;
        Self::parse_sps_from_elementary_stream(elementary_stream, stream_type)
    }

    /// Scan a single TS packet payload for SPS NAL units
    fn scan_payload_for_sps(payload: &[u8], stream_type: StreamType) -> Option<Resolution> {
        let nal_units = match stream_type {
            StreamType::H264 => Self::find_h264_sps_nal_units(payload),
            StreamType::H265 => Self::find_h265_sps_nal_units(payload),
            _ => Vec::new(),
        };

        for sps_data in nal_units {
            match stream_type {
                StreamType::H264 => {
                    if let Ok(sps) =
                        h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(sps_data))
                    {
                        return Some(Resolution::new(sps.width() as u32, sps.height() as u32));
                    }
                }
                StreamType::H265 => {
                    if let Ok(sps) = h265::SpsNALUnit::parse(std::io::Cursor::new(sps_data)) {
                        return Some(Resolution::new(
                            sps.rbsp.pic_width_in_luma_samples.get() as u32,
                            sps.rbsp.pic_height_in_luma_samples.get() as u32,
                        ));
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// Reassemble PES packets from TS packets
    fn reassemble_pes_from_ts_packets<'a>(
        ts_packets: impl Iterator<Item = &'a TsPacketRef>,
    ) -> Option<Bytes> {
        let mut pes_data = BytesMut::new();
        let mut in_pes_packet = false;

        for packet in ts_packets {
            if let Some(payload) = packet.payload() {
                if packet.payload_unit_start_indicator {
                    in_pes_packet = true;
                    pes_data.clear();
                    pes_data.extend_from_slice(&payload);
                } else if in_pes_packet {
                    pes_data.extend_from_slice(&payload);
                }
            }
        }

        if pes_data.is_empty() {
            None
        } else {
            Some(pes_data.freeze())
        }
    }

    /// Extract elementary stream data from PES packet
    fn extract_elementary_stream_from_pes(pes_data: &[u8]) -> Option<&[u8]> {
        if pes_data.len() < 9 || pes_data[0] != 0x00 || pes_data[1] != 0x00 || pes_data[2] != 0x01 {
            return None;
        }

        let pes_header_data_length = pes_data[8] as usize;
        let elementary_stream_start = 9 + pes_header_data_length;

        if elementary_stream_start >= pes_data.len() {
            None
        } else {
            Some(&pes_data[elementary_stream_start..])
        }
    }

    /// Parse SPS from elementary stream data
    fn parse_sps_from_elementary_stream(
        elementary_stream: &[u8],
        stream_type: StreamType,
    ) -> Option<Resolution> {
        match stream_type {
            StreamType::H264 => {
                let sps_nal_units = Self::find_h264_sps_nal_units(elementary_stream);
                for sps_data in sps_nal_units {
                    if let Ok(sps) =
                        h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(sps_data))
                    {
                        return Some(Resolution::new(sps.width() as u32, sps.height() as u32));
                    }
                }
            }
            StreamType::H265 => {
                let sps_nal_units = Self::find_h265_sps_nal_units(elementary_stream);
                for sps_data in sps_nal_units {
                    if let Ok(sps) = h265::SpsNALUnit::parse(std::io::Cursor::new(sps_data)) {
                        return Some(Resolution::new(
                            sps.rbsp.pic_width_in_luma_samples.get() as u32,
                            sps.rbsp.pic_height_in_luma_samples.get() as u32,
                        ));
                    }
                }
            }
            _ => {}
        }
        None
    }

    /// Find H.264 SPS NAL units in stream data
    fn find_h264_sps_nal_units(stream_data: &[u8]) -> Vec<&[u8]> {
        Self::find_nal_units_by_type(stream_data, |nal_header| (nal_header & 0x1F) == 0x07)
    }

    /// Find H.265 SPS NAL units in stream data
    fn find_h265_sps_nal_units(stream_data: &[u8]) -> Vec<&[u8]> {
        Self::find_nal_units_by_type(stream_data, |nal_header| ((nal_header & 0x7E) >> 1) == 33)
    }

    /// Find NAL units by type using a predicate function
    fn find_nal_units_by_type<F>(stream_data: &[u8], type_check: F) -> Vec<&[u8]>
    where
        F: Fn(u8) -> bool,
    {
        let mut nal_units = Vec::new();

        let mut search_pos = 0;
        while search_pos < stream_data.len() {
            let next_three = Self::three_byte_finder().find(&stream_data[search_pos..]);
            let next_four = Self::four_byte_finder().find(&stream_data[search_pos..]);

            let (start_code_pos, start_code_len) = match (next_three, next_four) {
                (Some(pos3), Some(pos4)) if pos4 <= pos3 => (pos4, 4),
                (Some(pos), _) => (pos, 3),
                (None, Some(pos)) => (pos, 4),
                (None, None) => break,
            };

            let nal_start = search_pos + start_code_pos + start_code_len;
            if nal_start >= stream_data.len() {
                break;
            }

            let nal_header = stream_data[nal_start];
            if type_check(nal_header) {
                // Find the end of this NAL unit by looking for the next start code
                let next_search_pos = nal_start + 1;
                let end_pos = if next_search_pos >= stream_data.len() {
                    stream_data.len()
                } else {
                    let next_three =
                        Self::three_byte_finder().find(&stream_data[next_search_pos..]);
                    let next_four = Self::four_byte_finder().find(&stream_data[next_search_pos..]);

                    match (next_three, next_four) {
                        (Some(pos3), Some(pos4)) if pos4 <= pos3 => next_search_pos + pos4,
                        (Some(pos), _) => next_search_pos + pos,
                        (None, Some(pos)) => next_search_pos + pos,
                        (None, None) => stream_data.len(),
                    }
                };

                nal_units.push(&stream_data[nal_start..end_pos]);
                search_pos = end_pos;
            } else {
                search_pos = nal_start;
            }
        }

        nal_units
    }

    /// Get default resolution based on codec type
    fn get_default_resolution(stream_type: &StreamType) -> Option<Resolution> {
        let (width, height) = match stream_type {
            StreamType::H264 => (1920, 1080), // Common HD resolution for H.264
            StreamType::H265 => (3840, 2160), // Common 4K resolution for H.265
            _ => (1280, 720),                 // Default HD-ready
        };

        Some(Resolution::new(width, height))
    }
}
