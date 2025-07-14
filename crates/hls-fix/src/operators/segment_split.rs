use crc32fast::Hasher;
use hls::segment::SegmentData;
use hls::{HlsData, M4sData};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// An operator that splits HLS segments when parameters change.
///
/// The SegmentSplitOperator performs deep inspection of stream metadata:
/// - MP4 initialization segment changes (different codecs, resolutions, etc.)
/// - TS segment PAT/PMT changes (program changes, PID changes, etc.)
/// - Stream parameter changes (resolution, codec, bitrate, etc.)
///
/// When meaningful changes are detected, the operator inserts an end marker
/// to properly split the HLS stream.
pub struct SegmentSplitOperator {
    context: Arc<StreamerContext>,
    last_init_segment_crc: Option<u32>,
    last_pat_crc: Option<u32>,
    last_pmt_crc: Option<u32>,
    program_map: HashMap<u16, u16>, // program_number -> PMT PID
    active_pmt_pid: Option<u16>,
}

// Constants for MPEG-TS parsing
const TS_PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const PAT_PID: u16 = 0x0000;
const PAT_TABLE_ID: u8 = 0x00;
const PMT_TABLE_ID: u8 = 0x02;

impl SegmentSplitOperator {
    /// Creates a new SegmentSplitOperator with the given context.
    ///
    /// # Arguments
    ///
    /// * `context` - The shared StreamerContext containing configuration and state
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            last_init_segment_crc: None,
            last_pat_crc: None,
            last_pmt_crc: None,
            program_map: HashMap::new(),
            active_pmt_pid: None,
        }
    }

    // Calculate CRC32 for byte content
    fn calculate_crc(data: &[u8]) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize()
    }

    // Handle MP4 init segment - returns true if a split is needed
    fn handle_init_segment(&mut self, input: &HlsData) -> Result<bool, PipelineError> {
        // Get data from HlsData
        let data = match input {
            HlsData::M4sData(init) => &init.data(),
            _ => {
                return Err(PipelineError::Processing(
                    "Expected MP4 init segment".to_string(),
                ));
            }
        };

        let crc = Self::calculate_crc(data);

        if let Some(previous_crc) = self.last_init_segment_crc {
            if previous_crc != crc {
                info!(
                    "{} Detected different init segment, splitting the stream",
                    self.context.name
                );
                self.last_init_segment_crc = Some(crc);
                // Signal that we need to emit an end marker before starting a new segment
                return Ok(true);
            }
        } else {
            // First init segment encountered
            self.last_init_segment_crc = Some(crc);
            info!("{} First init segment encountered", self.context.name);
        }

        // No split needed
        Ok(false)
    }

    // Handle TS segment by parsing PAT/PMT tables
    // Returns true if a split is needed
    fn handle_ts_segment(&mut self, input: &HlsData) -> Result<bool, PipelineError> {
        // Get data from HlsData
        let segment = match input {
            HlsData::TsData(segment) => segment,
            _ => {
                return Err(PipelineError::InvalidData("Expected TsSegment".to_string()));
            }
        };

        // Extract TS packets and parse PAT/PMT
        let data = &segment.data;

        // Check if we have PAT/PMT changes
        let mut pat_changed = false;
        let mut pmt_changed = false;

        // Iterate through TS packets (each 188 bytes)
        for chunk_start in (0..data.len()).step_by(TS_PACKET_SIZE) {
            // Make sure we have a complete packet
            if chunk_start + TS_PACKET_SIZE > data.len() {
                break;
            }

            let packet = &data[chunk_start..chunk_start + TS_PACKET_SIZE];

            // Check sync byte
            if packet[0] != SYNC_BYTE {
                continue;
            }

            // Extract PID (13 bits from bytes 1-2)
            let pid = (((packet[1] & 0x1F) as u16) << 8) | (packet[2] as u16);

            // Skip packets with adaptation field only or with payload scrambling
            let adaptation_field_control = (packet[3] & 0x30) >> 4;
            if adaptation_field_control == 0 || adaptation_field_control == 2 {
                continue;
            }

            // Check for payload unit start indicator
            let payload_start = (packet[1] & 0x40) != 0;
            if !payload_start {
                continue;
            }

            // Calculate payload offset
            let mut payload_offset = 4;

            // If adaptation field exists, skip it
            if (adaptation_field_control & 0x2) != 0 {
                let adaptation_field_length = packet[4] as usize;
                payload_offset += 1 + adaptation_field_length;

                // Make sure we don't exceed packet bounds
                if payload_offset >= TS_PACKET_SIZE {
                    continue;
                }
            }

            // Skip pointer field for sections
            payload_offset += 1;

            // Make sure we have enough bytes for a table
            if payload_offset + 3 >= TS_PACKET_SIZE {
                continue;
            }

            // Read table ID
            let table_id = packet[payload_offset];

            // Check if we're dealing with PAT or PMT tables
            if pid == PAT_PID && table_id == PAT_TABLE_ID {
                pat_changed = self.parse_pat(packet, payload_offset)?;
            } else if (self.active_pmt_pid == Some(pid)) && table_id == PMT_TABLE_ID {
                pmt_changed = self.parse_pmt(packet, payload_offset)?;
            }
        }

        // Return true if either PAT or PMT changed
        Ok(pat_changed || pmt_changed)
    }

    // Parse Program Association Table and check for changes
    fn parse_pat(&mut self, packet: &[u8], offset: usize) -> Result<bool, PipelineError> {
        // Need at least 8 bytes for a minimal PAT
        if offset + 8 >= packet.len() {
            return Ok(false);
        }

        // Section length (12 bits)
        let section_length =
            (((packet[offset + 1] & 0x0F) as usize) << 8) | (packet[offset + 2] as usize);

        // Make sure we have the full section
        if offset + 3 + section_length > packet.len() {
            return Ok(false);
        }

        // Calculate CRC for the PAT
        let pat_data = &packet[offset..offset + 3 + section_length];
        let crc = Self::calculate_crc(pat_data);

        let mut changed = false;

        if let Some(previous_crc) = self.last_pat_crc {
            if previous_crc != crc {
                info!(
                    "{} Detected PAT change, updating program map",
                    self.context.name
                );

                // Clear existing program map
                self.program_map.clear();

                // Parse program entries
                let mut pos = offset + 8; // Skip header
                while pos + 4 <= offset + 3 + section_length - 4 {
                    // Leave room for CRC
                    let program_number = ((packet[pos] as u16) << 8) | (packet[pos + 1] as u16);
                    let program_pid =
                        (((packet[pos + 2] & 0x1F) as u16) << 8) | (packet[pos + 3] as u16);

                    if program_number != 0 {
                        // Non-zero program number -> PMT
                        self.program_map.insert(program_number, program_pid);

                        // Use the first program's PMT as the active one
                        if self.active_pmt_pid.is_none() {
                            self.active_pmt_pid = Some(program_pid);
                        }
                    }

                    pos += 4;
                }

                self.last_pat_crc = Some(crc);
                changed = true;
            }
        } else {
            // First PAT encountered
            // Parse program entries as above
            let mut pos = offset + 8; // Skip header
            while pos + 4 <= offset + 3 + section_length - 4 {
                // Leave room for CRC
                let program_number = ((packet[pos] as u16) << 8) | (packet[pos + 1] as u16);
                let program_pid =
                    (((packet[pos + 2] & 0x1F) as u16) << 8) | (packet[pos + 3] as u16);

                if program_number != 0 {
                    // Non-zero program number -> PMT
                    self.program_map.insert(program_number, program_pid);

                    // Use the first program's PMT as the active one
                    if self.active_pmt_pid.is_none() {
                        self.active_pmt_pid = Some(program_pid);
                    }
                }

                pos += 4;
            }

            self.last_pat_crc = Some(crc);
            // First PAT doesn't trigger a split
        }

        Ok(changed)
    }

    // Parse Program Map Table and check for changes
    fn parse_pmt(&mut self, packet: &[u8], offset: usize) -> Result<bool, PipelineError> {
        // Need at least 12 bytes for a minimal PMT
        if offset + 12 >= packet.len() {
            return Ok(false);
        }

        // Section length (12 bits)
        let section_length =
            (((packet[offset + 1] & 0x0F) as usize) << 8) | (packet[offset + 2] as usize);

        // Make sure we have the full section
        if offset + 3 + section_length > packet.len() {
            return Ok(false);
        }

        // Calculate CRC for the PMT
        let pmt_data = &packet[offset..offset + 3 + section_length];
        let crc = Self::calculate_crc(pmt_data);

        let mut changed = false;

        if let Some(previous_crc) = self.last_pmt_crc {
            if previous_crc != crc {
                info!(
                    "{} Detected PMT change, stream parameters changed",
                    self.context.name
                );
                self.last_pmt_crc = Some(crc);
                changed = true;
            }
        } else {
            // First PMT encountered
            self.last_pmt_crc = Some(crc);
            // First PMT doesn't trigger a split
        }

        Ok(changed)
    }

    // Reset operator state
    fn reset(&mut self) {
        self.last_pat_crc = None;
        self.last_pmt_crc = None;
        self.last_init_segment_crc = None;
        self.program_map.clear();
        self.active_pmt_pid = None;
    }
}

impl Processor<HlsData> for SegmentSplitOperator {
    fn process(
        &mut self,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let mut need_split = false;

        // Check if we need to split based on segment type
        match &input {
            HlsData::M4sData(M4sData::InitSegment(_)) => {
                debug!("Init segment received");
                need_split = self.handle_init_segment(&input)?;
            }
            HlsData::TsData(_) => {
                need_split = self.handle_ts_segment(&input)?;
            }
            HlsData::EndMarker => {
                // Reset state when we see an end marker
                self.reset();
            }
            _ => {}
        }

        // If we need to split, emit an end marker first
        if need_split {
            debug!(
                "{} Emitting end marker for segment split",
                self.context.name
            );
            output(HlsData::end_marker())?;
        }

        // Always output the original input
        output(input)?;

        Ok(())
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        self.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SegmentSplitter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::{init_test_tracing, test_utils::create_test_context};

    // Helper function to create a basic PAT packet
    fn create_pat_packet(programs: &[(u16, u16)]) -> Vec<u8> {
        let mut packet = vec![0u8; TS_PACKET_SIZE];

        // Sync byte
        packet[0] = SYNC_BYTE;

        // Transport flags: payload unit start indicator + PAT PID
        packet[1] = 0x40; // Payload unit start
        packet[2] = 0x00; // PAT PID = 0

        // Adaptation control: payload only
        packet[3] = 0x10;

        // Pointer field
        packet[4] = 0x00;

        // PAT header
        packet[5] = PAT_TABLE_ID; // Table ID
        packet[6] = 0x80; // Section syntax indicator + reserved bits

        // Section length will be calculated later
        let section_length = 5 + (programs.len() * 4) + 4; // 5 bytes header, 4 bytes per program, 4 bytes CRC
        packet[6] |= ((section_length >> 8) & 0x0F) as u8;
        packet[7] = (section_length & 0xFF) as u8;

        // Transport stream ID
        packet[8] = 0x00;
        packet[9] = 0x01;

        // Version and section numbers
        packet[10] = 0xC1; // Reserved bits + version 0 + current indicator + section 0
        packet[11] = 0x00; // Last section number

        // Program entries
        let mut offset = 12;
        for (program_num, pid) in programs {
            packet[offset] = (*program_num >> 8) as u8;
            packet[offset + 1] = (*program_num & 0xFF) as u8;
            packet[offset + 2] = 0xE0 | ((*pid >> 8) & 0x1F) as u8;
            packet[offset + 3] = (*pid & 0xFF) as u8;
            offset += 4;
        }

        // CRC32 - in a real implementation we would calculate this
        // For testing, we'll just use a placeholder
        packet[offset..offset + 4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        packet
    }

    // Helper function to create a basic PMT packet
    fn create_pmt_packet(pmt_pid: u16, program_info: &[(u8, u16)]) -> Vec<u8> {
        let mut packet = vec![0u8; TS_PACKET_SIZE];

        // Sync byte
        packet[0] = SYNC_BYTE;

        // Transport flags: payload unit start indicator + PMT PID
        packet[1] = 0x40 | ((pmt_pid >> 8) & 0x1F) as u8;
        packet[2] = (pmt_pid & 0xFF) as u8;

        // Adaptation control: payload only
        packet[3] = 0x10;

        // Pointer field
        packet[4] = 0x00;

        // PMT header
        packet[5] = PMT_TABLE_ID; // Table ID
        packet[6] = 0x80; // Section syntax indicator + reserved bits

        // Section length will be calculated later
        let section_length = 9 + (program_info.len() * 5) + 4; // 9 bytes header, 5 bytes per stream, 4 bytes CRC
        packet[6] |= ((section_length >> 8) & 0x0F) as u8;
        packet[7] = (section_length & 0xFF) as u8;

        // Program number
        packet[8] = 0x00;
        packet[9] = 0x01;

        // Version and section numbers
        packet[10] = 0xC1; // Reserved bits + version 0 + current indicator + section 0

        // PCR PID
        packet[11] = 0xE0 | ((pmt_pid >> 8) & 0x1F) as u8;
        packet[12] = (pmt_pid & 0xFF) as u8;

        // Program info length (0 for now)
        packet[13] = 0xF0;
        packet[14] = 0x00;

        // Stream entries
        let mut offset = 15;
        for (stream_type, pid) in program_info {
            packet[offset] = *stream_type;
            packet[offset + 1] = 0xE0 | ((*pid >> 8) & 0x1F) as u8;
            packet[offset + 2] = (*pid & 0xFF) as u8;
            packet[offset + 3] = 0xF0; // Reserved bits
            packet[offset + 4] = 0x00; // ES info length = 0
            offset += 5;
        }

        // CRC32 - in a real implementation we would calculate this
        // For testing, we'll just use a placeholder
        packet[offset..offset + 4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        packet
    }

    #[test]
    fn test_pat_change_detection() {
        let context = create_test_context();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Create initial PAT with two programs
        let programs1 = vec![(1, 0x1000), (2, 0x1001)];
        let pat1 = create_pat_packet(&programs1);

        // Create a TS segment with the initial PAT
        let ts_segment1 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(pat1),
        });

        // Process the initial PAT
        operator.process(ts_segment1, &mut output_fn).unwrap();

        // Create second PAT with different programs
        let programs2 = vec![(1, 0x1000), (3, 0x1002)]; // Program 2 -> 3
        let pat2 = create_pat_packet(&programs2);

        // Create a TS segment with the modified PAT
        let ts_segment2 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(pat2),
        });

        // Process the modified PAT
        operator.process(ts_segment2, &mut output_fn).unwrap();

        // Should have split the stream (end marker + new segment)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker"),
        }
    }

    #[test]
    fn test_pmt_change_detection() {
        let context = create_test_context();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // First create a PAT to set up the PMT PID
        let pmt_pid = 0x1000;
        let programs = vec![(1, pmt_pid)];
        let pat = create_pat_packet(&programs);

        // Create initial PMT with audio and video
        let streams1 = vec![(0x1B, 0x1001), (0x0F, 0x1002)]; // H.264 video + AAC audio
        let pmt1 = create_pmt_packet(pmt_pid, &streams1);

        // Create a TS segment with PAT and PMT
        let mut combined1 = pat.clone();
        combined1.extend_from_slice(&pmt1);

        let ts_segment1 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(combined1),
        });

        // Process the initial PAT+PMT
        operator.process(ts_segment1, &mut output_fn).unwrap();

        // Create second PMT with changed streams
        let streams2 = vec![(0x24, 0x1001), (0x0F, 0x1002)]; // H.265 video + AAC audio
        let pmt2 = create_pmt_packet(pmt_pid, &streams2);

        // Create a TS segment with PAT and modified PMT
        let mut combined2 = pat.clone();
        combined2.extend_from_slice(&pmt2);

        let ts_segment2 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(combined2),
        });

        // Process the modified PMT
        operator.process(ts_segment2, &mut output_fn).unwrap();

        // Should have split the stream (end marker + new segment)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker"),
        }
    }

    #[test]
    fn test_init_segment_change() {
        init_test_tracing!();
        let context = create_test_context();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Create initial init segment
        let init1 = HlsData::mp4_init(MediaSegment::empty(), Bytes::from(vec![1, 2, 3, 4]));

        // Process the initial init segment
        operator.process(init1, &mut output_fn).unwrap();
        // Create a different init segment
        let init2 = HlsData::mp4_init(MediaSegment::empty(), Bytes::from(vec![5, 6, 7, 8]));

        // Process the new init segment
        operator.process(init2, &mut output_fn).unwrap();

        // Should have split the stream (end marker + new segment)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker"),
        }
    }
}
