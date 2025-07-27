use crate::{Result, StreamType, TsError};
use bytes::{Buf, Bytes};
use memchr::memchr;
use std::collections::HashMap;

/// Zero-copy TS packet parser
#[derive(Debug, Clone)]
pub struct TsPacketRef {
    /// Source packet data (exactly 188 bytes)
    data: Bytes,
    /// Parsed header information
    pub sync_byte: u8,
    pub transport_error_indicator: bool,
    pub payload_unit_start_indicator: bool,
    pub transport_priority: bool,
    pub pid: u16,
    pub transport_scrambling_control: u8,
    pub adaptation_field_control: u8,
    pub continuity_counter: u8,
    /// Offset to adaptation field (if present)
    adaptation_field_offset: Option<usize>,
    /// Offset to payload (if present)  
    payload_offset: Option<usize>,
}

impl TsPacketRef {
    /// Parse a TS packet from 188 bytes
    pub fn parse(data: Bytes) -> Result<Self> {
        if data.len() != 188 {
            return Err(TsError::InvalidPacketSize(data.len()));
        }
        let mut reader = &data[..];
        let sync_byte = reader.get_u8();
        if sync_byte != 0x47 {
            return Err(TsError::InvalidSyncByte(sync_byte));
        }
        let byte1 = reader.get_u8();
        let byte2 = reader.get_u8();
        let byte3 = reader.get_u8();
        let transport_error_indicator = (byte1 & 0x80) != 0;
        let payload_unit_start_indicator = (byte1 & 0x40) != 0;
        let transport_priority = (byte1 & 0x20) != 0;
        let pid = ((byte1 as u16 & 0x1F) << 8) | byte2 as u16;
        let transport_scrambling_control = (byte3 >> 6) & 0x03;
        let adaptation_field_control = (byte3 >> 4) & 0x03;
        let continuity_counter = byte3 & 0x0F;
        let mut offset = 4;
        let mut adaptation_field_offset = None;
        let mut payload_offset = None;
        // Calculate adaptation field offset
        if adaptation_field_control == 0x02 || adaptation_field_control == 0x03 {
            if offset >= data.len() {
                return Err(TsError::InsufficientData {
                    expected: offset + 1,
                    actual: data.len(),
                });
            }
            let adaptation_field_length = data[offset] as usize;
            adaptation_field_offset = Some(offset);
            offset += 1 + adaptation_field_length;
        }
        // Calculate payload offset
        if (adaptation_field_control == 0x01 || adaptation_field_control == 0x03)
            && offset < data.len()
        {
            payload_offset = Some(offset);
        }
        Ok(TsPacketRef {
            data,
            sync_byte,
            transport_error_indicator,
            payload_unit_start_indicator,
            transport_priority,
            pid,
            transport_scrambling_control,
            adaptation_field_control,
            continuity_counter,
            adaptation_field_offset,
            payload_offset,
        })
    }
    /// Get adaptation field data
    #[inline]
    pub fn adaptation_field(&self) -> Option<Bytes> {
        if let Some(offset) = self.adaptation_field_offset {
            if offset + 1 < self.data.len() {
                let length = self.data[offset] as usize;
                if offset + 1 + length <= self.data.len() {
                    return Some(self.data.slice(offset + 1..offset + 1 + length));
                }
            }
        }
        None
    }
    /// Get payload data
    #[inline]
    pub fn payload(&self) -> Option<Bytes> {
        if let Some(offset) = self.payload_offset {
            if offset < self.data.len() {
                return Some(self.data.slice(offset..));
            }
        }
        None
    }
    /// Get PSI payload (removes pointer field if PUSI is set)
    pub fn psi_payload(&self) -> Option<Bytes> {
        if let Some(payload) = self.payload() {
            if self.payload_unit_start_indicator && !payload.is_empty() {
                let pointer_field = payload[0] as usize;
                if 1 + pointer_field < payload.len() {
                    return Some(payload.slice(1 + pointer_field..));
                }
            } else if !self.payload_unit_start_indicator {
                return Some(payload);
            }
        }
        None
    }

    /// Check if this packet has a random access indicator
    pub fn has_random_access_indicator(&self) -> bool {
        if let Some(adaptation_field) = self.adaptation_field() {
            if !adaptation_field.is_empty() {
                return (adaptation_field[0] & 0x40) != 0;
            }
        }
        false
    }
}

/// Zero-copy PAT parser
#[derive(Debug, Clone)]
pub struct PatRef {
    /// Source PSI section data
    data: Bytes,
    /// Parsed header info (lightweight)
    pub table_id: u8,
    pub transport_stream_id: u16,
    pub version_number: u8,
    pub current_next_indicator: bool,
    pub section_number: u8,
    pub last_section_number: u8,
    /// Offset to programs section
    programs_offset: usize,
    programs_length: usize,
}

impl PatRef {
    /// Parse PAT from PSI section data
    pub fn parse(data: Bytes) -> Result<Self> {
        if data.len() < 8 {
            return Err(TsError::InsufficientData {
                expected: 8,
                actual: data.len(),
            });
        }
        let mut reader = &data[..];
        let table_id = reader.get_u8();
        if table_id != 0x00 {
            return Err(TsError::InvalidTableId {
                expected: 0x00,
                actual: table_id,
            });
        }
        let byte1 = reader.get_u8();
        let section_syntax_indicator = (byte1 & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PAT must have section syntax indicator set".to_string(),
            ));
        }
        let section_length = ((byte1 as u16 & 0x0F) << 8) | reader.get_u8() as u16;
        if section_length < 9 {
            return Err(TsError::InvalidSectionLength(section_length));
        }
        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }
        let transport_stream_id = reader.get_u16();
        let byte5 = reader.get_u8();
        let version_number = (byte5 >> 1) & 0x1F;
        let current_next_indicator = (byte5 & 0x01) != 0;
        let section_number = reader.get_u8();
        let last_section_number = reader.get_u8();
        let programs_offset = 8;
        let programs_end = 3 + section_length as usize - 4; // Exclude CRC32
        let programs_length = programs_end - programs_offset;
        Ok(PatRef {
            data,
            table_id,
            transport_stream_id,
            version_number,
            current_next_indicator,
            section_number,
            last_section_number,
            programs_offset,
            programs_length,
        })
    }

    /// Iterator over programs without allocating
    pub fn programs(&self) -> PatProgramIterator {
        PatProgramIterator {
            data: self
                .data
                .slice(self.programs_offset..self.programs_offset + self.programs_length),
        }
    }

    /// Get program count efficiently
    pub fn program_count(&self) -> usize {
        self.programs_length / 4
    }
}

/// Iterator over PAT programs that doesn't allocate
#[derive(Debug)]
pub struct PatProgramIterator {
    data: Bytes,
}

impl Iterator for PatProgramIterator {
    type Item = PatProgramRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.remaining() >= 4 {
            let program_number = self.data.get_u16();
            let pmt_pid = ((self.data.get_u8() as u16 & 0x1F) << 8) | self.data.get_u8() as u16;
            Some(PatProgramRef {
                program_number,
                pmt_pid,
            })
        } else {
            None
        }
    }
}

/// Zero-copy PAT program entry
#[derive(Debug, Clone, Copy)]
pub struct PatProgramRef {
    pub program_number: u16,
    pub pmt_pid: u16,
}

/// Zero-copy PMT parser
#[derive(Debug, Clone)]
pub struct PmtRef {
    /// Source PSI section data
    data: Bytes,
    /// Parsed header info
    pub table_id: u8,
    pub program_number: u16,
    pub version_number: u8,
    pub current_next_indicator: bool,
    pub section_number: u8,
    pub last_section_number: u8,
    pub pcr_pid: u16,
    /// Program info descriptors (reference to source)
    program_info_offset: usize,
    program_info_length: usize,
    /// Elementary streams section
    streams_offset: usize,
    streams_length: usize,
}

impl PmtRef {
    /// Parse PMT from PSI section data
    pub fn parse(data: Bytes) -> Result<Self> {
        if data.len() < 12 {
            return Err(TsError::InsufficientData {
                expected: 12,
                actual: data.len(),
            });
        }
        let mut reader = &data[..];
        let table_id = reader.get_u8();
        if table_id != 0x02 {
            return Err(TsError::InvalidTableId {
                expected: 0x02,
                actual: table_id,
            });
        }
        let byte1 = reader.get_u8();
        let section_syntax_indicator = (byte1 & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PMT must have section syntax indicator set".to_string(),
            ));
        }
        let section_length = ((byte1 as u16 & 0x0F) << 8) | reader.get_u8() as u16;
        if section_length < 13 {
            return Err(TsError::InvalidSectionLength(section_length));
        }
        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }
        let program_number = reader.get_u16();
        let byte5 = reader.get_u8();
        let version_number = (byte5 >> 1) & 0x1F;
        let current_next_indicator = (byte5 & 0x01) != 0;
        let section_number = reader.get_u8();
        let last_section_number = reader.get_u8();
        let pcr_pid_high = reader.get_u8();
        let pcr_pid_low = reader.get_u8();
        let pcr_pid = ((pcr_pid_high as u16 & 0x1F) << 8) | pcr_pid_low as u16;

        let prog_info_len_high = reader.get_u8();
        let prog_info_len_low = reader.get_u8();
        let program_info_length =
            (((prog_info_len_high as u16) & 0x0F) << 8) | prog_info_len_low as u16;
        let program_info_length = program_info_length as usize;

        if (section_length as usize) < 9 + program_info_length + 4 {
            return Err(TsError::InvalidSectionLength(section_length));
        }

        let program_info_offset = 12;
        let streams_offset = 12 + program_info_length;
        let streams_end = 3 + section_length as usize - 4; // Exclude CRC32
        let streams_length = streams_end - streams_offset;
        Ok(PmtRef {
            data,
            table_id,
            program_number,
            version_number,
            current_next_indicator,
            section_number,
            last_section_number,
            pcr_pid,
            program_info_offset,
            program_info_length,
            streams_offset,
            streams_length,
        })
    }

    /// Get program info descriptors
    #[inline]
    pub fn program_info(&self) -> Bytes {
        self.data
            .slice(self.program_info_offset..self.program_info_offset + self.program_info_length)
    }

    /// Iterator over elementary streams without allocating
    pub fn streams(&self) -> PmtStreamIterator {
        PmtStreamIterator {
            data: self
                .data
                .slice(self.streams_offset..self.streams_offset + self.streams_length),
        }
    }
}

/// Iterator over PMT streams that doesn't allocate
#[derive(Debug)]
pub struct PmtStreamIterator {
    data: Bytes,
}

impl Iterator for PmtStreamIterator {
    type Item = Result<PmtStreamRef>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.remaining() >= 5 {
            let mut reader = &self.data[..];
            let stream_type = StreamType::from(reader.get_u8());
            let elementary_pid = ((reader.get_u8() as u16 & 0x1F) << 8) | reader.get_u8() as u16;
            let es_info_length =
                (((reader.get_u8() as u16 & 0x0F) << 8) | reader.get_u8() as u16) as usize;
            self.data.advance(5);

            if self.data.remaining() < es_info_length {
                return Some(Err(TsError::InsufficientData {
                    expected: es_info_length,
                    actual: self.data.remaining(),
                }));
            }

            let es_info = self.data.split_to(es_info_length);

            Some(Ok(PmtStreamRef {
                stream_type,
                elementary_pid,
                es_info,
            }))
        } else {
            None
        }
    }
}

/// Zero-copy PMT stream entry
#[derive(Debug, Clone)]
pub struct PmtStreamRef {
    pub stream_type: StreamType,
    pub elementary_pid: u16,
    pub es_info: Bytes,
}

/// Zero-copy streaming TS parser with minimal memory footprint
#[derive(Debug, Default)]
pub struct TsParser {
    /// Program mapping: program_number -> pmt_pid
    program_pids: HashMap<u16, u16>,
    /// Reverse PMT PID lookup: pmt_pid -> program_number
    pmt_pids: HashMap<u16, u16>,
    /// Current version numbers to detect updates
    pat_version: Option<u8>,
    pmt_versions: HashMap<u16, u8>, // program_number -> version
}

impl TsParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse TS packets with zero-copy approach and call handlers for found PSI
    pub fn parse_packets<F, G, H>(
        &mut self,
        mut data: Bytes,
        mut on_pat: F,
        mut on_pmt: G,
        mut on_packet: Option<H>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        H: FnMut(&TsPacketRef) -> Result<()>,
    {
        while !data.is_empty() {
            // Fast path: if we're already at a sync byte, we don't need to search
            if data.len() >= 188 && data[0] == 0x47 {
                // We have a sync byte and enough data for a packet
            } else {
                // Slow path: search for the next sync byte
                if let Some(sync_offset) = memchr(0x47, &data) {
                    data.advance(sync_offset);
                } else {
                    // No more sync bytes in the buffer. Advance to the end to avoid repeated scans.
                    data.advance(data.len());
                    break;
                }
            }

            if data.len() < 188 {
                // Not enough data for a full packet
                break;
            }

            // At this point, data[0] is 0x47.
            let chunk = data.slice(0..188);
            if let Ok(packet) = TsPacketRef::parse(chunk) {
                // Successfully parsed a packet.
                if let Some(on_packet_cb) = &mut on_packet {
                    on_packet_cb(&packet)?;
                }

                if packet.payload_unit_start_indicator {
                    if let Some(psi_payload) = packet.psi_payload() {
                        self.process_psi_payload(
                            packet.pid,
                            psi_payload,
                            &mut on_pat,
                            &mut on_pmt,
                        )?;
                    }
                }
                data.advance(188);
            } else {
                // The packet was invalid despite the sync byte.
                // Advance one byte to continue searching from the next position.
                data.advance(1);
            }
        }
        Ok(())
    }

    /// Process a PSI payload from a packet
    fn process_psi_payload<F, G>(
        &mut self,
        pid: u16,
        psi_payload: Bytes,
        on_pat: &mut F,
        on_pmt: &mut G,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
    {
        if pid == 0x0000 {
            if let Ok(pat) = PatRef::parse(psi_payload) {
                self.process_pat(pat, on_pat)?;
            }
        } else if self.pmt_pids.contains_key(&pid) {
            // It could be a PAT on a PMT PID, check table_id
            if psi_payload.is_empty() {
                return Ok(());
            }
            match psi_payload[0] {
                0x00 => {
                    // PAT packet on a PMT PID, re-process PAT
                    if let Ok(pat) = PatRef::parse(psi_payload) {
                        self.process_pat(pat, on_pat)?;
                    }
                }
                0x02 => {
                    // PMT packet
                    if let Ok(pmt) = PmtRef::parse(psi_payload) {
                        let program_number = self.pmt_pids.get(&pid).cloned().unwrap_or(0);
                        let is_new = self
                            .pmt_versions
                            .get(&program_number)
                            .is_none_or(|&v| v != pmt.version_number);
                        if is_new {
                            self.pmt_versions.insert(program_number, pmt.version_number);
                            on_pmt(pmt)?;
                        }
                    }
                }
                _ => {
                    // Unknown table ID on a PMT PID, ignore
                }
            }
        }
        Ok(())
    }

    /// Process a parsed PAT
    fn process_pat<F>(&mut self, pat: PatRef, on_pat: &mut F) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
    {
        let is_new = self.pat_version != Some(pat.version_number);
        if is_new {
            self.pat_version = Some(pat.version_number);

            // A new PAT version has been received, clear all program-related state.
            self.program_pids.clear();
            self.pmt_pids.clear();
            self.pmt_versions.clear();

            // Populate the maps with the new program data.
            for program in pat.programs() {
                if program.program_number != 0 {
                    self.program_pids
                        .insert(program.program_number, program.pmt_pid);
                    self.pmt_pids
                        .insert(program.pmt_pid, program.program_number);
                }
            }

            on_pat(pat)?;
        }
        Ok(())
    }

    /// Reset parser state
    pub fn reset(&mut self) {
        self.program_pids.clear();
        self.pmt_pids.clear();
        self.pat_version = None;
        self.pmt_versions.clear();
    }

    /// Get estimated memory usage for the parser (for debugging/profiling)
    pub fn estimated_memory_usage(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.program_pids.capacity() * (std::mem::size_of::<u16>() * 2)
            + self.pmt_pids.capacity() * (std::mem::size_of::<u16>() * 2)
            + self.pmt_versions.capacity()
                * (std::mem::size_of::<u16>() + std::mem::size_of::<u8>())
    }

    /// Get number of tracked programs (for debugging)
    pub fn program_count(&self) -> usize {
        self.program_pids.len()
    }
}
