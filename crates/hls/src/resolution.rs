use bytes::Bytes;
use tracing::debug;
use ts::StreamType;

/// Represents video resolution information
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Information extracted from a TS packet for resolution detection
#[derive(Debug, Clone)]
struct TsPacketInfo {
    pid: u16,
    payload_unit_start_indicator: bool,
    payload: Option<Bytes>,
}

/// Resolution detector for HLS segments
pub struct ResolutionDetector;

impl ResolutionDetector {
    /// Extract resolution from TS data using a tiered approach
    /// 1. Try simple TS packet scanning first (fast)
    /// 2. Fall back to PES reassembly if needed (reliable)
    /// 3. Use codec-based defaults as last resort
    pub fn extract_from_ts_data(
        ts_data: &Bytes,
        video_streams: &[(u16, StreamType)],
    ) -> Option<Resolution> {
        if video_streams.is_empty() {
            return None;
        }

        // Try simple scanning first (most common case)
        for (pid, stream_type) in video_streams {
            if let Some(resolution) = Self::try_simple_scanning(ts_data, *pid, *stream_type) {
                debug!(
                    "Found resolution {}x{} via simple scanning for PID 0x{:04X} {:?}",
                    resolution.width, resolution.height, pid, stream_type
                );
                return Some(resolution);
            }
        }

        // If simple scanning fails, try PES reassembly (handles fragmented SPS)
        for (pid, stream_type) in video_streams {
            if let Some(resolution) = Self::try_pes_reassembly(ts_data, *pid, *stream_type) {
                debug!(
                    "Found resolution {}x{} via PES reassembly for PID 0x{:04X} {:?}",
                    resolution.width, resolution.height, pid, stream_type
                );
                return Some(resolution);
            }
        }

        // Fallback to typical resolutions based on codec
        Self::get_default_resolution(&video_streams[0].1)
    }

    /// Quick scanning of TS packet payloads for SPS (works when SPS fits in single packet)
    fn try_simple_scanning(
        ts_data: &Bytes,
        video_pid: u16,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        let data = ts_data.as_ref();
        let mut offset = 0;

        while offset + 188 <= data.len() {
            let packet_data = &data[offset..offset + 188];

            if packet_data[0] == 0x47 {
                let pid = ((packet_data[1] as u16 & 0x1F) << 8) | (packet_data[2] as u16);

                if pid == video_pid {
                    if let Some(payload) = Self::extract_ts_payload(packet_data) {
                        if let Some(resolution) = Self::scan_payload_for_sps(&payload, stream_type)
                        {
                            return Some(resolution);
                        }
                    }
                }
            }

            offset += 188;
        }

        None
    }

    /// Full PES reassembly approach (handles fragmented SPS across multiple TS packets)
    fn try_pes_reassembly(
        ts_data: &Bytes,
        video_pid: u16,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        let ts_packets = Self::parse_ts_packets(ts_data)?;

        let video_packets: Vec<_> = ts_packets
            .into_iter()
            .filter(|packet| packet.pid == video_pid)
            .collect();

        if video_packets.is_empty() {
            return None;
        }

        let pes_data = Self::reassemble_pes_from_ts_packets(&video_packets)?;
        let elementary_stream = Self::extract_elementary_stream_from_pes(&pes_data)?;
        Self::parse_sps_from_elementary_stream(&elementary_stream, stream_type)
    }

    /// Extract payload from a single TS packet
    fn extract_ts_payload(packet_data: &[u8]) -> Option<Bytes> {
        if packet_data.len() != 188 || packet_data[0] != 0x47 {
            return None;
        }

        let adaptation_field_control = (packet_data[3] & 0x30) >> 4;

        let mut payload_start = 4;
        if adaptation_field_control == 2 || adaptation_field_control == 3 {
            let adaptation_field_length = packet_data[4] as usize;
            payload_start = 5 + adaptation_field_length;
        }

        if adaptation_field_control == 1 || adaptation_field_control == 3 {
            if payload_start < 188 {
                Some(Bytes::copy_from_slice(&packet_data[payload_start..]))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Scan a single TS packet payload for SPS NAL units
    fn scan_payload_for_sps(payload: &Bytes, stream_type: StreamType) -> Option<Resolution> {
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
                        let width = sps.width();
                        let height = sps.height();
                        return Some(Resolution::new(width as u32, height as u32));
                    }
                }
                StreamType::H265 => {
                    if let Ok(sps) = h265::Sps::parse(sps_data) {
                        return Some(Resolution::new(sps.width as u32, sps.height as u32));
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// Parse TS data into individual TS packets
    fn parse_ts_packets(ts_data: &Bytes) -> Option<Vec<TsPacketInfo>> {
        let data = ts_data.as_ref();
        let mut packets = Vec::new();
        let mut offset = 0;

        while offset + 188 <= data.len() {
            let packet_data = &data[offset..offset + 188];

            if packet_data[0] != 0x47 {
                offset += 1;
                continue;
            }

            let transport_error_indicator = (packet_data[1] & 0x80) != 0;
            let payload_unit_start_indicator = (packet_data[1] & 0x40) != 0;
            let pid = ((packet_data[1] as u16 & 0x1F) << 8) | (packet_data[2] as u16);
            let adaptation_field_control = (packet_data[3] & 0x30) >> 4;

            if transport_error_indicator {
                offset += 188;
                continue;
            }

            let mut payload_start = 4;
            if adaptation_field_control == 2 || adaptation_field_control == 3 {
                let adaptation_field_length = packet_data[4] as usize;
                payload_start = 5 + adaptation_field_length;
            }

            let payload = if adaptation_field_control == 1 || adaptation_field_control == 3 {
                if payload_start < 188 {
                    Some(Bytes::copy_from_slice(&packet_data[payload_start..]))
                } else {
                    None
                }
            } else {
                None
            };

            packets.push(TsPacketInfo {
                pid,
                payload_unit_start_indicator,
                payload,
            });

            offset += 188;
        }

        if packets.is_empty() {
            None
        } else {
            Some(packets)
        }
    }

    /// Reassemble PES packets from TS packets
    fn reassemble_pes_from_ts_packets(ts_packets: &Vec<TsPacketInfo>) -> Option<Bytes> {
        let mut pes_data = Vec::new();
        let mut in_pes_packet = false;

        for packet in ts_packets {
            if let Some(ref payload) = packet.payload {
                if packet.payload_unit_start_indicator {
                    in_pes_packet = true;
                    pes_data.clear();
                    pes_data.extend_from_slice(payload);
                } else if in_pes_packet {
                    pes_data.extend_from_slice(payload);
                }
            }
        }

        if pes_data.is_empty() {
            None
        } else {
            Some(Bytes::from(pes_data))
        }
    }

    /// Extract elementary stream data from PES packet
    fn extract_elementary_stream_from_pes(pes_data: &Bytes) -> Option<Bytes> {
        let data = pes_data.as_ref();

        if data.len() < 9 || data[0] != 0x00 || data[1] != 0x00 || data[2] != 0x01 {
            return None;
        }

        let pes_header_data_length = data[8] as usize;
        let elementary_stream_start = 9 + pes_header_data_length;

        if elementary_stream_start >= data.len() {
            None
        } else {
            Some(Bytes::copy_from_slice(&data[elementary_stream_start..]))
        }
    }

    /// Parse SPS from elementary stream data
    fn parse_sps_from_elementary_stream(
        elementary_stream: &Bytes,
        stream_type: StreamType,
    ) -> Option<Resolution> {
        match stream_type {
            StreamType::H264 => {
                let sps_nal_units = Self::find_h264_sps_nal_units(elementary_stream);
                for sps_data in sps_nal_units {
                    if let Ok(sps) =
                        h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(sps_data))
                    {
                        let width = sps.width();
                        let height = sps.height();
                        return Some(Resolution::new(width as u32, height as u32));
                    }
                }
            }
            StreamType::H265 => {
                let sps_nal_units = Self::find_h265_sps_nal_units(elementary_stream);
                for sps_data in sps_nal_units {
                    if let Ok(sps) = h265::Sps::parse(sps_data) {
                        return Some(Resolution::new(sps.width as u32, sps.height as u32));
                    }
                }
            }
            _ => {}
        }
        None
    }

    /// Find H.264 SPS NAL units in stream data
    fn find_h264_sps_nal_units(stream_data: &Bytes) -> Vec<Bytes> {
        Self::find_nal_units_by_type(stream_data, |nal_header| {
            (nal_header & 0x1F) == 0x07 // H.264 SPS NAL type
        })
    }

    /// Find H.265 SPS NAL units in stream data
    fn find_h265_sps_nal_units(stream_data: &Bytes) -> Vec<Bytes> {
        Self::find_nal_units_by_type(stream_data, |nal_header| {
            ((nal_header & 0x7E) >> 1) == 33 // H.265 SPS NAL type
        })
    }

    /// Find NAL units by type using a predicate function
    fn find_nal_units_by_type<F>(stream_data: &Bytes, type_check: F) -> Vec<Bytes>
    where
        F: Fn(u8) -> bool,
    {
        let data = stream_data.as_ref();
        let mut nal_units = Vec::new();

        // Check if data is too small to contain any NAL units
        if data.len() < 4 {
            return nal_units;
        }

        let mut i = 0;
        let search_limit = data.len().saturating_sub(4);

        while i < search_limit {
            if data[i] == 0x00 && data[i + 1] == 0x00 {
                let start_code_len = if data[i + 2] == 0x00 && data[i + 3] == 0x01 {
                    4 // 0x00000001
                } else if data[i + 2] == 0x01 {
                    3 // 0x000001
                } else {
                    i += 1;
                    continue;
                };

                if i + start_code_len < data.len() {
                    let nal_header = data[i + start_code_len];

                    if type_check(nal_header) {
                        let start = i + start_code_len;
                        let mut end = start;
                        let end_search_limit = data.len().saturating_sub(3);

                        while end < end_search_limit {
                            if data[end] == 0x00
                                && data[end + 1] == 0x00
                                && (data[end + 2] == 0x01
                                    || (data[end + 2] == 0x00
                                        && end + 3 < data.len()
                                        && data[end + 3] == 0x01))
                            {
                                break;
                            }
                            end += 1;
                        }

                        // If we reached the end without finding another start code, include remaining data
                        if end == end_search_limit {
                            end = data.len();
                        }

                        if end > start {
                            nal_units.push(Bytes::copy_from_slice(&data[start..end]));
                        }
                    }
                }
            }
            i += 1;
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
