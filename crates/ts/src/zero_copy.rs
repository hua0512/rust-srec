use crate::{Result, StreamType, TsError};
use std::collections::HashMap;

/// Zero-copy TS packet parser that references source data
#[derive(Debug)]
pub struct TsPacketRef<'data> {
    /// Source packet data (exactly 188 bytes)
    data: &'data [u8],
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

impl<'data> TsPacketRef<'data> {
    /// Parse a TS packet from 188 bytes without copying
    pub fn parse(data: &'data [u8]) -> Result<Self> {
        if data.len() != 188 {
            return Err(TsError::InvalidPacketSize(data.len()));
        }

        let sync_byte = data[0];
        if sync_byte != 0x47 {
            return Err(TsError::InvalidSyncByte(sync_byte));
        }

        let byte1 = data[1];
        let byte2 = data[2];
        let byte3 = data[3];

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

    /// Get adaptation field data without copying
    #[inline]
    pub fn adaptation_field(&self) -> Option<&'data [u8]> {
        if let Some(offset) = self.adaptation_field_offset {
            if offset + 1 < self.data.len() {
                let length = self.data[offset] as usize;
                if offset + 1 + length <= self.data.len() {
                    return Some(&self.data[offset + 1..offset + 1 + length]);
                }
            }
        }
        None
    }

    /// Get payload data without copying
    #[inline]
    pub fn payload(&self) -> Option<&'data [u8]> {
        if let Some(offset) = self.payload_offset {
            if offset < self.data.len() {
                return Some(&self.data[offset..]);
            }
        }
        None
    }

    /// Get PSI payload without copying (removes pointer field if PUSI is set)
    pub fn psi_payload(&self) -> Option<&'data [u8]> {
        if let Some(payload) = self.payload() {
            if self.payload_unit_start_indicator && !payload.is_empty() {
                let pointer_field = payload[0] as usize;
                if 1 + pointer_field < payload.len() {
                    return Some(&payload[1 + pointer_field..]);
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

/// Zero-copy PAT parser that references source data
#[derive(Debug, Clone)]
pub struct PatRef<'data> {
    /// Source PSI section data
    data: &'data [u8],
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

impl<'data> PatRef<'data> {
    /// Parse PAT from PSI section data without copying
    pub fn parse(data: &'data [u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(TsError::InsufficientData {
                expected: 8,
                actual: data.len(),
            });
        }

        let table_id = data[0];
        if table_id != 0x00 {
            return Err(TsError::InvalidTableId {
                expected: 0x00,
                actual: table_id,
            });
        }

        let section_syntax_indicator = (data[1] & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PAT must have section syntax indicator set".to_string(),
            ));
        }

        let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
        if section_length < 5 {
            return Err(TsError::InvalidSectionLength(section_length));
        }

        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }

        let transport_stream_id = ((data[3] as u16) << 8) | data[4] as u16;
        let version_number = (data[5] >> 1) & 0x1F;
        let current_next_indicator = (data[5] & 0x01) != 0;
        let section_number = data[6];
        let last_section_number = data[7];

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
    pub fn programs(&self) -> PatProgramIterator<'data> {
        PatProgramIterator {
            data: &self.data[self.programs_offset..self.programs_offset + self.programs_length],
            offset: 0,
        }
    }

    /// Get program count efficiently
    pub fn program_count(&self) -> usize {
        self.programs_length / 4
    }
}

/// Iterator over PAT programs that doesn't allocate
#[derive(Debug)]
pub struct PatProgramIterator<'data> {
    data: &'data [u8],
    offset: usize,
}

impl<'data> Iterator for PatProgramIterator<'data> {
    type Item = PatProgramRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + 4 <= self.data.len() {
            let program_number =
                ((self.data[self.offset] as u16) << 8) | self.data[self.offset + 1] as u16;
            let pmt_pid = ((self.data[self.offset + 2] as u16 & 0x1F) << 8)
                | self.data[self.offset + 3] as u16;

            self.offset += 4;

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

/// Zero-copy PMT parser that references source data
#[derive(Debug, Clone)]
pub struct PmtRef<'data> {
    /// Source PSI section data
    data: &'data [u8],
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

impl<'data> PmtRef<'data> {
    /// Parse PMT from PSI section data without copying
    pub fn parse(data: &'data [u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(TsError::InsufficientData {
                expected: 12,
                actual: data.len(),
            });
        }

        let table_id = data[0];
        if table_id != 0x02 {
            return Err(TsError::InvalidTableId {
                expected: 0x02,
                actual: table_id,
            });
        }

        let section_syntax_indicator = (data[1] & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PMT must have section syntax indicator set".to_string(),
            ));
        }

        let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
        if section_length < 9 {
            return Err(TsError::InvalidSectionLength(section_length));
        }

        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }

        let program_number = ((data[3] as u16) << 8) | data[4] as u16;
        let version_number = (data[5] >> 1) & 0x1F;
        let current_next_indicator = (data[5] & 0x01) != 0;
        let section_number = data[6];
        let last_section_number = data[7];
        let pcr_pid = ((data[8] as u16 & 0x1F) << 8) | data[9] as u16;

        let program_info_length = ((data[10] as u16 & 0x0F) << 8) | data[11] as u16;
        let program_info_offset = 12;

        let streams_offset = 12 + program_info_length as usize;
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
            program_info_length: program_info_length as usize,
            streams_offset,
            streams_length,
        })
    }

    /// Get program info descriptors without copying
    #[inline]
    pub fn program_info(&self) -> &'data [u8] {
        &self.data[self.program_info_offset..self.program_info_offset + self.program_info_length]
    }

    /// Iterator over elementary streams without allocating
    pub fn streams(&self) -> PmtStreamIterator<'data> {
        PmtStreamIterator {
            data: &self.data[self.streams_offset..self.streams_offset + self.streams_length],
            offset: 0,
        }
    }
}

/// Iterator over PMT streams that doesn't allocate
#[derive(Debug)]
pub struct PmtStreamIterator<'data> {
    data: &'data [u8],
    offset: usize,
}

impl<'data> Iterator for PmtStreamIterator<'data> {
    type Item = Result<PmtStreamRef<'data>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + 5 <= self.data.len() {
            let stream_type = StreamType::from(self.data[self.offset]);
            let elementary_pid = ((self.data[self.offset + 1] as u16 & 0x1F) << 8)
                | self.data[self.offset + 2] as u16;
            let es_info_length = ((self.data[self.offset + 3] as u16 & 0x0F) << 8)
                | self.data[self.offset + 4] as u16;

            let es_info_offset = self.offset + 5;
            if es_info_offset + es_info_length as usize > self.data.len() {
                return Some(Err(TsError::InsufficientData {
                    expected: es_info_offset + es_info_length as usize,
                    actual: self.data.len(),
                }));
            }

            self.offset += 5 + es_info_length as usize;

            Some(Ok(PmtStreamRef {
                stream_type,
                elementary_pid,
                es_info: &self.data[es_info_offset..es_info_offset + es_info_length as usize],
            }))
        } else {
            None
        }
    }
}

/// Zero-copy PMT stream entry
#[derive(Debug, Clone)]
pub struct PmtStreamRef<'data> {
    pub stream_type: StreamType,
    pub elementary_pid: u16,
    pub es_info: &'data [u8],
}

/// Zero-copy streaming TS parser with minimal memory footprint
#[derive(Debug, Default)]
pub struct ZeroCopyTsParser {
    /// Only store essential program mapping info
    program_pids: HashMap<u16, u16>, // program_number -> pmt_pid
    /// Current version numbers to detect updates
    pat_version: Option<u8>,
    pmt_versions: HashMap<u16, u8>, // program_number -> version
}

impl ZeroCopyTsParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse TS packets with zero-copy approach and call handlers for found PSI
    pub fn parse_packets<F, G>(&mut self, data: &[u8], mut on_pat: F, mut on_pmt: G) -> Result<()>
    where
        F: FnMut(PatRef<'_>) -> Result<()>,
        G: FnMut(PmtRef<'_>) -> Result<()>,
    {
        if data.len() % 188 != 0 {
            return Err(TsError::InvalidPacketSize(data.len()));
        }

        for chunk in data.chunks_exact(188) {
            let packet = TsPacketRef::parse(chunk)?;

            match packet.pid {
                0x0000 => {
                    // PAT
                    if let Some(psi_payload) = packet.psi_payload() {
                        let pat = PatRef::parse(psi_payload)?;

                        // Check if this is a new version
                        let is_new = self.pat_version != Some(pat.version_number);
                        if is_new {
                            self.pat_version = Some(pat.version_number);

                            // Update program mapping
                            self.program_pids.clear();
                            for program in pat.programs() {
                                if program.program_number != 0 {
                                    self.program_pids
                                        .insert(program.program_number, program.pmt_pid);
                                }
                            }

                            on_pat(pat)?;
                        }
                    }
                }
                pid => {
                    // Check if this is a PMT PID
                    if let Some(program_number) = self
                        .program_pids
                        .iter()
                        .find(|(_prog_num, pmt_pid)| **pmt_pid == pid)
                        .map(|(prog_num, _pmt_pid)| *prog_num)
                    {
                        if let Some(psi_payload) = packet.psi_payload() {
                            let pmt = PmtRef::parse(psi_payload)?;

                            // Check if this is a new version
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
                }
            }
        }

        Ok(())
    }

    /// Reset parser state
    pub fn reset(&mut self) {
        self.program_pids.clear();
        self.pat_version = None;
        self.pmt_versions.clear();
    }

    /// Get estimated memory usage for the parser (for debugging/profiling)
    pub fn estimated_memory_usage(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.program_pids.capacity() * (std::mem::size_of::<u16>() * 2)
            + self.pmt_versions.capacity()
                * (std::mem::size_of::<u16>() + std::mem::size_of::<u8>())
    }

    /// Get number of tracked programs (for debugging)
    pub fn program_count(&self) -> usize {
        self.program_pids.len()
    }
}
