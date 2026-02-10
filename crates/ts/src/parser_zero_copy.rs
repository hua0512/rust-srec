use crate::{ContinuityMode, Result, StreamType, TsError};
use bytes::{Buf, Bytes, BytesMut};
use memchr::memchr_iter;
use std::collections::{HashMap, HashSet};

const PID_SPACE: usize = 8192;

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
        if let Some(offset) = self.adaptation_field_offset
            && offset + 1 < self.data.len()
        {
            let length = self.data[offset] as usize;
            if offset + 1 + length <= self.data.len() {
                return Some(self.data.slice(offset + 1..offset + 1 + length));
            }
        }
        None
    }
    /// Get payload data
    #[inline]
    pub fn payload(&self) -> Option<Bytes> {
        if let Some(offset) = self.payload_offset
            && offset < self.data.len()
        {
            return Some(self.data.slice(offset..));
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
        if let Some(adaptation_field) = self.adaptation_field()
            && !adaptation_field.is_empty()
        {
            return (adaptation_field[0] & 0x40) != 0;
        }
        false
    }

    /// Parse the adaptation field into a structured type.
    pub fn parse_adaptation_field(&self) -> Option<crate::adaptation_field::AdaptationFieldRef> {
        self.adaptation_field()
            .and_then(crate::adaptation_field::AdaptationFieldRef::parse)
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

    /// Parse PAT from PSI section data with CRC-32/MPEG-2 validation.
    pub fn parse_with_crc(data: Bytes) -> Result<Self> {
        if data.len() >= 7 {
            let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
            let section_end = 3 + section_length as usize;
            if section_end <= data.len()
                && section_end >= 4
                && !crate::crc32::validate_section_crc32(&data[..section_end])
            {
                let stored = u32::from_be_bytes([
                    data[section_end - 4],
                    data[section_end - 3],
                    data[section_end - 2],
                    data[section_end - 1],
                ]);
                let calculated = crate::crc32::mpeg2_crc32(&data[..section_end - 4]);
                return Err(TsError::Crc32Mismatch {
                    expected: stored,
                    calculated,
                });
            }
        }
        Self::parse(data)
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

    /// Parse PMT from PSI section data with CRC-32/MPEG-2 validation.
    pub fn parse_with_crc(data: Bytes) -> Result<Self> {
        if data.len() >= 7 {
            let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
            let section_end = 3 + section_length as usize;
            if section_end <= data.len()
                && section_end >= 4
                && !crate::crc32::validate_section_crc32(&data[..section_end])
            {
                let stored = u32::from_be_bytes([
                    data[section_end - 4],
                    data[section_end - 3],
                    data[section_end - 2],
                    data[section_end - 1],
                ]);
                let calculated = crate::crc32::mpeg2_crc32(&data[..section_end - 4]);
                return Err(TsError::Crc32Mismatch {
                    expected: stored,
                    calculated,
                });
            }
        }
        Self::parse(data)
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

    /// Iterate over program info descriptors.
    pub fn program_descriptors(&self) -> crate::descriptor::DescriptorIterator {
        crate::descriptor::DescriptorIterator::new(self.program_info())
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

impl PmtStreamRef {
    /// Iterate over ES info descriptors.
    pub fn descriptors(&self) -> crate::descriptor::DescriptorIterator {
        crate::descriptor::DescriptorIterator::new(self.es_info.clone())
    }
}

/// Zero-copy streaming TS parser with minimal memory footprint
#[derive(Debug)]
pub struct TsParser {
    /// Program mapping: program_number -> pmt_pid
    program_pids: HashMap<u16, u16>,
    /// Reverse PMT PID lookup: pmt_pid -> program_number
    pmt_pids: HashMap<u16, u16>,
    /// Fast PMT PID membership table
    pmt_pid_flags: [bool; PID_SPACE],
    /// Current version numbers to detect updates
    pat_version: Option<u8>,
    pmt_versions: HashMap<u16, u8>, // program_number -> version
    /// Whether to validate CRC-32/MPEG-2 on PAT/PMT sections
    validate_crc: bool,
    /// Last continuity counter value for each PID
    continuity_counters: [u8; PID_SPACE],
    /// Whether a PID has seen at least one packet
    continuity_seen: [bool; PID_SPACE],
    /// Continuity counter handling mode
    continuity_mode: ContinuityMode,
    /// Number of continuity issues seen while parsing
    continuity_issue_count: usize,
    /// Number of duplicate continuity counter issues seen while parsing
    continuity_duplicate_count: usize,
    /// Number of discontinuity continuity counter issues seen while parsing
    continuity_discontinuity_count: usize,
    /// Detected SCTE-35 PIDs (from PMT registration descriptors)
    scte35_pids: HashSet<u16>,
    /// Fast SCTE-35 PID membership table
    scte35_pid_flags: [bool; PID_SPACE],
    /// Buffers for incomplete PSI sections, keyed by PID
    psi_buffers: HashMap<u16, BytesMut>,
}

impl Default for TsParser {
    fn default() -> Self {
        Self {
            program_pids: HashMap::new(),
            pmt_pids: HashMap::new(),
            pmt_pid_flags: [false; PID_SPACE],
            pat_version: None,
            pmt_versions: HashMap::new(),
            validate_crc: false,
            continuity_counters: [0; PID_SPACE],
            continuity_seen: [false; PID_SPACE],
            continuity_mode: ContinuityMode::Disabled,
            continuity_issue_count: 0,
            continuity_duplicate_count: 0,
            continuity_discontinuity_count: 0,
            scte35_pids: HashSet::new(),
            scte35_pid_flags: [false; PID_SPACE],
            psi_buffers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PacketFormat {
    Ts188,
    M2ts192,
    Ts204,
}

impl PacketFormat {
    const fn packet_size(self) -> usize {
        match self {
            Self::Ts188 => 188,
            Self::M2ts192 => 192,
            Self::Ts204 => 204,
        }
    }

    const fn sync_offset(self) -> usize {
        match self {
            Self::Ts188 => 0,
            Self::M2ts192 => 4,
            Self::Ts204 => 0,
        }
    }
}

impl TsParser {
    const MAX_PSI_SECTION_LENGTH: usize = 0x0FFF;
    const MAX_PSI_BUFFER_SIZE: usize = 64 * 1024;
    const PACKET_FORMATS: [PacketFormat; 3] = [
        PacketFormat::Ts188,
        PacketFormat::M2ts192,
        PacketFormat::Ts204,
    ];

    pub fn new() -> Self {
        Self::default()
    }

    fn find_sync(data: &Bytes) -> Option<(usize, PacketFormat)> {
        for sync_pos in memchr_iter(0x47, data.as_ref()) {
            for format in Self::PACKET_FORMATS {
                let sync_offset = format.sync_offset();
                if sync_pos < sync_offset {
                    continue;
                }

                let offset = sync_pos - sync_offset;
                if Self::packet_starts_at(data, offset, format) {
                    return Some((offset, format));
                }
            }
        }
        None
    }

    fn packet_starts_at(data: &Bytes, offset: usize, format: PacketFormat) -> bool {
        let packet_size = format.packet_size();
        let sync_offset = format.sync_offset();

        if offset + packet_size > data.len() {
            return false;
        }

        let first_sync = offset + sync_offset;

        if first_sync >= data.len() || data[first_sync] != 0x47 {
            return false;
        }

        let second_sync = first_sync + packet_size;
        if second_sync < data.len() {
            data[second_sync] == 0x47
        } else {
            true
        }
    }

    fn slice_packet_payload(data: &Bytes, format: PacketFormat) -> Option<Bytes> {
        let packet_size = format.packet_size();
        let sync_offset = format.sync_offset();
        if data.len() < packet_size {
            return None;
        }

        let sync_pos = sync_offset;
        if data[sync_pos] != 0x47 {
            return None;
        }

        let packet_end = sync_pos + 188;
        if packet_end > data.len() {
            return None;
        }

        Some(data.slice(sync_pos..packet_end))
    }

    fn handle_continuity_status(
        &mut self,
        pid: u16,
        status: crate::packet::ContinuityStatus,
    ) -> Result<()> {
        use crate::packet::ContinuityStatus;

        match status {
            ContinuityStatus::Initial | ContinuityStatus::Ok => Ok(()),
            ContinuityStatus::Duplicate => {
                self.continuity_issue_count += 1;
                self.continuity_duplicate_count += 1;
                match self.continuity_mode {
                    ContinuityMode::Disabled => Ok(()),
                    ContinuityMode::Warn => Ok(()),
                    ContinuityMode::Strict => Err(TsError::DuplicatePacket {
                        pid,
                        cc: self.continuity_counters[pid as usize],
                    }),
                }
            }
            ContinuityStatus::Discontinuity { expected, actual } => {
                self.continuity_issue_count += 1;
                self.continuity_discontinuity_count += 1;
                match self.continuity_mode {
                    ContinuityMode::Disabled => Ok(()),
                    ContinuityMode::Warn => Ok(()),
                    ContinuityMode::Strict => Err(TsError::ContinuityError {
                        pid,
                        expected,
                        actual,
                    }),
                }
            }
        }
    }

    /// Enable or disable CRC-32/MPEG-2 validation on PAT/PMT sections.
    pub fn with_crc_validation(mut self, enable: bool) -> Self {
        self.validate_crc = enable;
        self
    }

    /// Set continuity counter handling mode.
    pub fn with_continuity_mode(mut self, mode: ContinuityMode) -> Self {
        self.continuity_mode = mode;
        self
    }

    /// Enable or disable continuity counter checking.
    pub fn with_continuity_check(mut self, enable: bool) -> Self {
        self.continuity_mode = if enable {
            ContinuityMode::Warn
        } else {
            ContinuityMode::Disabled
        };
        self
    }

    /// Number of continuity issues observed during parsing.
    pub fn continuity_issue_count(&self) -> usize {
        self.continuity_issue_count
    }

    /// Number of duplicate continuity issues observed during parsing.
    pub fn continuity_duplicate_count(&self) -> usize {
        self.continuity_duplicate_count
    }

    /// Number of discontinuity continuity issues observed during parsing.
    pub fn continuity_discontinuity_count(&self) -> usize {
        self.continuity_discontinuity_count
    }

    /// Check continuity counter for a packet. Returns the status.
    fn check_cc(&mut self, packet: &TsPacketRef) -> crate::packet::ContinuityStatus {
        use crate::packet::ContinuityStatus;

        // Skip null packets
        if packet.pid == crate::packet::PID_NULL {
            return ContinuityStatus::Ok;
        }

        let pid_idx = packet.pid as usize;
        if pid_idx >= PID_SPACE {
            return ContinuityStatus::Ok;
        }

        let has_payload =
            packet.adaptation_field_control == 0x01 || packet.adaptation_field_control == 0x03;

        if self.continuity_seen[pid_idx] {
            let last_cc = self.continuity_counters[pid_idx];
            if has_payload {
                let expected = (last_cc + 1) & 0x0F;
                if packet.continuity_counter == expected {
                    self.continuity_counters[pid_idx] = packet.continuity_counter;
                    ContinuityStatus::Ok
                } else if packet.continuity_counter == last_cc {
                    ContinuityStatus::Duplicate
                } else {
                    self.continuity_counters[pid_idx] = packet.continuity_counter;
                    ContinuityStatus::Discontinuity {
                        expected,
                        actual: packet.continuity_counter,
                    }
                }
            } else {
                // Adaptation-only: CC should not change
                if packet.continuity_counter == last_cc {
                    ContinuityStatus::Ok
                } else {
                    ContinuityStatus::Discontinuity {
                        expected: last_cc,
                        actual: packet.continuity_counter,
                    }
                }
            }
        } else {
            self.continuity_seen[pid_idx] = true;
            self.continuity_counters[pid_idx] = packet.continuity_counter;
            ContinuityStatus::Initial
        }
    }

    /// Parse TS packets with zero-copy approach and call handlers for found PSI
    pub fn parse_packets<F, G, H>(
        &mut self,
        data: Bytes,
        on_pat: F,
        on_pmt: G,
        on_packet: Option<H>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        H: FnMut(&TsPacketRef) -> Result<()>,
    {
        self.parse_packets_inner(
            data,
            on_pat,
            on_pmt,
            on_packet,
            None::<fn(crate::scte35::SpliceInfoSectionRef) -> Result<()>>,
        )
    }

    /// Parse TS packets with SCTE-35 splice information support.
    ///
    /// SCTE-35 PIDs are auto-detected from PMT entries with a registration
    /// descriptor containing format identifier "CUEI".
    pub fn parse_packets_with_scte35<F, G, H, S>(
        &mut self,
        data: Bytes,
        on_pat: F,
        on_pmt: G,
        on_packet: Option<H>,
        on_scte35: S,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        H: FnMut(&TsPacketRef) -> Result<()>,
        S: FnMut(crate::scte35::SpliceInfoSectionRef) -> Result<()>,
    {
        self.parse_packets_inner(data, on_pat, on_pmt, on_packet, Some(on_scte35))
    }

    fn parse_packets_inner<F, G, H, S>(
        &mut self,
        mut data: Bytes,
        mut on_pat: F,
        mut on_pmt: G,
        mut on_packet: Option<H>,
        mut on_scte35: Option<S>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        H: FnMut(&TsPacketRef) -> Result<()>,
        S: FnMut(crate::scte35::SpliceInfoSectionRef) -> Result<()>,
    {
        self.continuity_issue_count = 0;
        self.continuity_duplicate_count = 0;
        self.continuity_discontinuity_count = 0;
        let mut locked_format: Option<PacketFormat> = None;

        while !data.is_empty() {
            let packet_format = if let Some(format) = locked_format {
                if Self::packet_starts_at(&data, 0, format) {
                    format
                } else {
                    let (sync_offset, discovered_format) =
                        if let Some(found) = Self::find_sync(&data) {
                            found
                        } else {
                            break;
                        };

                    if sync_offset > 0 {
                        data.advance(sync_offset);
                    }

                    locked_format = Some(discovered_format);
                    discovered_format
                }
            } else {
                let (sync_offset, discovered_format) = if let Some(found) = Self::find_sync(&data) {
                    found
                } else {
                    break;
                };

                if sync_offset > 0 {
                    data.advance(sync_offset);
                }

                locked_format = Some(discovered_format);
                discovered_format
            };

            let packet_size = packet_format.packet_size();
            if data.len() < packet_size {
                // Not enough data for a full packet
                break;
            }

            let Some(chunk) = Self::slice_packet_payload(&data, packet_format) else {
                locked_format = None;
                data.advance(1);
                continue;
            };

            if let Ok(packet) = TsPacketRef::parse(chunk) {
                // Check continuity counter if enabled
                if self.continuity_mode != ContinuityMode::Disabled {
                    let status = self.check_cc(&packet);
                    self.handle_continuity_status(packet.pid, status)?;
                }

                // Successfully parsed a packet.
                if let Some(on_packet_cb) = &mut on_packet {
                    on_packet_cb(&packet)?;
                }

                if self.is_relevant_psi_pid(packet.pid)
                    && let Some(payload) = packet.payload()
                {
                    self.process_packet_psi_payload(
                        packet.pid,
                        payload,
                        packet.payload_unit_start_indicator,
                        &mut on_pat,
                        &mut on_pmt,
                        &mut on_scte35,
                    )?;
                }
                data.advance(packet_size);
            } else {
                // The packet was invalid despite the sync byte.
                // Advance one byte to continue searching from the next position.
                locked_format = None;
                data.advance(1);
            }
        }
        Ok(())
    }

    #[inline]
    fn is_relevant_psi_pid(&self, pid: u16) -> bool {
        let pid_idx = pid as usize;
        pid == 0x0000
            || (pid_idx < PID_SPACE
                && (self.pmt_pid_flags[pid_idx] || self.scte35_pid_flags[pid_idx]))
    }

    fn process_packet_psi_payload<F, G, S>(
        &mut self,
        pid: u16,
        payload: Bytes,
        payload_unit_start_indicator: bool,
        on_pat: &mut F,
        on_pmt: &mut G,
        on_scte35: &mut Option<S>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        S: FnMut(crate::scte35::SpliceInfoSectionRef) -> Result<()>,
    {
        if payload.is_empty() {
            return Ok(());
        }

        if payload_unit_start_indicator {
            let pointer_field = payload[0] as usize;
            let pointer_end = 1 + pointer_field;
            if pointer_end > payload.len() {
                return Ok(());
            }

            if pointer_field > 0 {
                self.append_psi_bytes(pid, &payload[1..pointer_end], on_pat, on_pmt, on_scte35)?;
            }

            if pointer_end < payload.len() {
                if let Some(buffer) = self.psi_buffers.get_mut(&pid)
                    && !buffer.is_empty()
                {
                    buffer.clear();
                }
                self.append_psi_bytes(pid, &payload[pointer_end..], on_pat, on_pmt, on_scte35)?;
            }
        } else {
            self.append_psi_bytes(pid, &payload, on_pat, on_pmt, on_scte35)?;
        }

        Ok(())
    }

    fn append_psi_bytes<F, G, S>(
        &mut self,
        pid: u16,
        data: &[u8],
        on_pat: &mut F,
        on_pmt: &mut G,
        on_scte35: &mut Option<S>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        S: FnMut(crate::scte35::SpliceInfoSectionRef) -> Result<()>,
    {
        if data.is_empty() {
            return Ok(());
        }

        let sections = {
            let buffer = self.psi_buffers.entry(pid).or_default();
            buffer.extend_from_slice(data);
            if buffer.len() > Self::MAX_PSI_BUFFER_SIZE {
                buffer.clear();
                return Ok(());
            }
            let mut sections = Vec::new();

            loop {
                let stuffing_prefix = buffer.iter().take_while(|&&b| b == 0xFF).count();
                if stuffing_prefix > 0 {
                    let _ = buffer.split_to(stuffing_prefix);
                }

                if buffer.len() < 3 {
                    break;
                }

                let section_length = (((buffer[1] as usize) & 0x0F) << 8) | buffer[2] as usize;
                if section_length > Self::MAX_PSI_SECTION_LENGTH {
                    let _ = buffer.split_to(1);
                    continue;
                }

                let section_size = 3 + section_length;
                if buffer.len() < section_size {
                    break;
                }

                sections.push(buffer.split_to(section_size).freeze());
            }

            sections
        };

        for section in sections {
            self.process_psi_payload_inner(pid, section, on_pat, on_pmt, on_scte35)?;
        }

        Ok(())
    }

    /// Internal PSI payload processing with optional SCTE-35 support.
    fn process_psi_payload_inner<F, G, S>(
        &mut self,
        pid: u16,
        psi_payload: Bytes,
        on_pat: &mut F,
        on_pmt: &mut G,
        on_scte35: &mut Option<S>,
    ) -> Result<()>
    where
        F: FnMut(PatRef) -> Result<()>,
        G: FnMut(PmtRef) -> Result<()>,
        S: FnMut(crate::scte35::SpliceInfoSectionRef) -> Result<()>,
    {
        if pid == 0x0000 {
            let parse_result = if self.validate_crc {
                PatRef::parse_with_crc(psi_payload)
            } else {
                PatRef::parse(psi_payload)
            };
            if let Ok(pat) = parse_result {
                self.process_pat(pat, on_pat)?;
            }
        } else if (pid as usize) < PID_SPACE && self.scte35_pid_flags[pid as usize] {
            // SCTE-35 splice info
            if let Some(on_scte35_cb) = on_scte35
                && !psi_payload.is_empty()
                && psi_payload[0] == crate::scte35::SCTE35_TABLE_ID
                && let Ok(section) = crate::scte35::SpliceInfoSectionRef::parse(psi_payload)
            {
                on_scte35_cb(section)?;
            }
        } else if (pid as usize) < PID_SPACE && self.pmt_pid_flags[pid as usize] {
            // It could be a PAT on a PMT PID, check table_id
            if psi_payload.is_empty() {
                return Ok(());
            }
            match psi_payload[0] {
                0x00 => {
                    // PAT packet on a PMT PID, re-process PAT
                    let parse_result = if self.validate_crc {
                        PatRef::parse_with_crc(psi_payload)
                    } else {
                        PatRef::parse(psi_payload)
                    };
                    if let Ok(pat) = parse_result {
                        self.process_pat(pat, on_pat)?;
                    }
                }
                0x02 => {
                    // PMT packet
                    let parse_result = if self.validate_crc {
                        PmtRef::parse_with_crc(psi_payload)
                    } else {
                        PmtRef::parse(psi_payload)
                    };
                    if let Ok(pmt) = parse_result {
                        let program_number = self.pmt_pids.get(&pid).copied().unwrap_or(0);
                        let is_new = self
                            .pmt_versions
                            .get(&program_number)
                            .is_none_or(|&v| v != pmt.version_number);
                        if is_new {
                            self.pmt_versions.insert(program_number, pmt.version_number);
                            // Detect SCTE-35 PIDs from this PMT
                            self.detect_scte35_pids(&pmt);
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

    /// Detect SCTE-35 PIDs from a PMT by looking for streams with
    /// registration descriptor format identifier "CUEI".
    fn detect_scte35_pids(&mut self, pmt: &PmtRef) {
        for stream in pmt.streams().flatten() {
            // Check ES info descriptors for registration descriptor "CUEI"
            for desc in stream.descriptors() {
                if desc.tag == crate::descriptor::TAG_REGISTRATION
                    && let Some(format_id) =
                        crate::descriptor::parse_registration_descriptor(&desc.data)
                    && &format_id == b"CUEI"
                {
                    self.scte35_pids.insert(stream.elementary_pid);
                    let pid_idx = stream.elementary_pid as usize;
                    if pid_idx < PID_SPACE {
                        self.scte35_pid_flags[pid_idx] = true;
                    }
                }
            }
        }
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
            self.pmt_pid_flags = [false; PID_SPACE];
            self.pmt_versions.clear();
            self.scte35_pids.clear();
            self.scte35_pid_flags = [false; PID_SPACE];
            self.psi_buffers.clear();

            // Populate the maps with the new program data.
            for program in pat.programs() {
                if program.program_number != 0 {
                    self.program_pids
                        .insert(program.program_number, program.pmt_pid);
                    self.pmt_pids
                        .insert(program.pmt_pid, program.program_number);
                    let pid_idx = program.pmt_pid as usize;
                    if pid_idx < PID_SPACE {
                        self.pmt_pid_flags[pid_idx] = true;
                    }
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
        self.pmt_pid_flags = [false; PID_SPACE];
        self.pat_version = None;
        self.pmt_versions.clear();
        self.continuity_counters = [0; PID_SPACE];
        self.continuity_seen = [false; PID_SPACE];
        self.continuity_issue_count = 0;
        self.continuity_duplicate_count = 0;
        self.continuity_discontinuity_count = 0;
        self.scte35_pids.clear();
        self.scte35_pid_flags = [false; PID_SPACE];
        self.psi_buffers.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_ts_packet(
        pid: u16,
        payload_unit_start_indicator: bool,
        cc: u8,
        payload: &[u8],
    ) -> Vec<u8> {
        assert!(payload.len() <= 184);
        let mut packet = vec![0xFFu8; 188];
        packet[0] = 0x47;
        packet[1] = ((pid >> 8) & 0x1F) as u8;
        if payload_unit_start_indicator {
            packet[1] |= 0x40;
        }
        packet[2] = (pid & 0xFF) as u8;
        packet[3] = 0x10 | (cc & 0x0F);
        packet[4..4 + payload.len()].copy_from_slice(payload);
        packet
    }

    fn build_pat_section(version: u8, program_count: usize, first_pmt_pid: u16) -> Vec<u8> {
        let section_length = 9 + program_count * 4;
        assert!(section_length <= 0x0FFF);

        let mut section = Vec::with_capacity(3 + section_length);
        section.push(0x00);
        section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F));
        section.push((section_length & 0xFF) as u8);
        section.push(0x00);
        section.push(0x01);
        section.push(0xC0 | ((version & 0x1F) << 1) | 0x01);
        section.push(0x00);
        section.push(0x00);

        for i in 0..program_count {
            let program_number = (i as u16) + 1;
            let pmt_pid = first_pmt_pid + i as u16;
            section.push((program_number >> 8) as u8);
            section.push((program_number & 0xFF) as u8);
            section.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
            section.push((pmt_pid & 0xFF) as u8);
        }

        section.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        section
    }

    fn build_pmt_section(
        version: u8,
        program_number: u16,
        pcr_pid: u16,
        stream_count: usize,
        first_stream_pid: u16,
    ) -> Vec<u8> {
        let section_length = 13 + stream_count * 5;
        assert!(section_length <= 0x0FFF);

        let mut section = Vec::with_capacity(3 + section_length);
        section.push(0x02);
        section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F));
        section.push((section_length & 0xFF) as u8);
        section.push((program_number >> 8) as u8);
        section.push((program_number & 0xFF) as u8);
        section.push(0xC0 | ((version & 0x1F) << 1) | 0x01);
        section.push(0x00);
        section.push(0x00);
        section.push(0xE0 | ((pcr_pid >> 8) as u8 & 0x1F));
        section.push((pcr_pid & 0xFF) as u8);
        section.push(0xF0);
        section.push(0x00);

        for i in 0..stream_count {
            let stream_pid = first_stream_pid + i as u16;
            section.push(0x1B);
            section.push(0xE0 | ((stream_pid >> 8) as u8 & 0x1F));
            section.push((stream_pid & 0xFF) as u8);
            section.push(0xF0);
            section.push(0x00);
        }

        section.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        section
    }

    #[test]
    fn reassembles_pat_section_across_packets() {
        let pat_section = build_pat_section(0, 50, 0x0100);
        let split_at = 183;

        let mut payload_1 = Vec::with_capacity(184);
        payload_1.push(0x00);
        payload_1.extend_from_slice(&pat_section[..split_at]);

        let payload_2 = pat_section[split_at..].to_vec();

        let packet_1 = build_ts_packet(0x0000, true, 0, &payload_1);
        let packet_2 = build_ts_packet(0x0000, false, 1, &payload_2);

        let mut stream = Vec::new();
        stream.extend_from_slice(&packet_1);
        stream.extend_from_slice(&packet_2);

        let mut parser = TsParser::new();
        let mut pat_count = 0usize;
        let mut programs = 0usize;

        parser
            .parse_packets(
                Bytes::from(stream),
                |pat| {
                    pat_count += 1;
                    programs = pat.programs().count();
                    Ok(())
                },
                |_pmt| Ok(()),
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(pat_count, 1);
        assert_eq!(programs, 50);
    }

    #[test]
    fn reassembles_pmt_section_across_packets() {
        let pat_section = build_pat_section(0, 1, 0x0100);
        let mut pat_payload = Vec::with_capacity(184);
        pat_payload.push(0x00);
        pat_payload.extend_from_slice(&pat_section);

        let pmt_section = build_pmt_section(0, 1, 0x0101, 40, 0x0101);
        let split_at = 183;

        let mut pmt_payload_1 = Vec::with_capacity(184);
        pmt_payload_1.push(0x00);
        pmt_payload_1.extend_from_slice(&pmt_section[..split_at]);

        let pmt_payload_2 = pmt_section[split_at..].to_vec();

        let packet_pat = build_ts_packet(0x0000, true, 0, &pat_payload);
        let packet_pmt_1 = build_ts_packet(0x0100, true, 0, &pmt_payload_1);
        let packet_pmt_2 = build_ts_packet(0x0100, false, 1, &pmt_payload_2);

        let mut stream = Vec::new();
        stream.extend_from_slice(&packet_pat);
        stream.extend_from_slice(&packet_pmt_1);
        stream.extend_from_slice(&packet_pmt_2);

        let mut parser = TsParser::new();
        let mut pmt_count = 0usize;
        let mut stream_count = 0usize;

        parser
            .parse_packets(
                Bytes::from(stream),
                |_pat| Ok(()),
                |pmt| {
                    pmt_count += 1;
                    stream_count = pmt.streams().flatten().count();
                    Ok(())
                },
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(pmt_count, 1);
        assert_eq!(stream_count, 40);
    }

    #[test]
    fn handles_pointer_field_completing_previous_section() {
        let pat_v0 = build_pat_section(0, 1, 0x0100);
        let pat_v1 = build_pat_section(1, 1, 0x0100);

        let split_at = 6;
        let mut payload_1 = Vec::with_capacity(184);
        payload_1.push(0x00);
        payload_1.extend_from_slice(&pat_v0[..split_at]);

        let remainder_v0 = &pat_v0[split_at..];
        let mut payload_2 = Vec::new();
        payload_2.push(remainder_v0.len() as u8);
        payload_2.extend_from_slice(remainder_v0);
        payload_2.extend_from_slice(&pat_v1);

        let packet_1 = build_ts_packet(0x0000, true, 0, &payload_1);
        let packet_2 = build_ts_packet(0x0000, true, 1, &payload_2);

        let mut stream = Vec::new();
        stream.extend_from_slice(&packet_1);
        stream.extend_from_slice(&packet_2);

        let mut parser = TsParser::new();
        let mut versions = Vec::new();

        parser
            .parse_packets(
                Bytes::from(stream),
                |pat| {
                    versions.push(pat.version_number);
                    Ok(())
                },
                |_pmt| Ok(()),
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(versions, vec![0, 1]);
    }

    #[test]
    fn continuity_warn_mode_reports_issues_without_failing() {
        let packet_1 = build_ts_packet(0x0100, false, 0, &[0x00]);
        let packet_2 = build_ts_packet(0x0100, false, 5, &[0x01]);

        let mut stream = Vec::new();
        stream.extend_from_slice(&packet_1);
        stream.extend_from_slice(&packet_2);

        let mut parser = TsParser::new().with_continuity_mode(ContinuityMode::Warn);
        let result = parser.parse_packets(
            Bytes::from(stream),
            |_pat| Ok(()),
            |_pmt| Ok(()),
            None::<fn(&TsPacketRef) -> Result<()>>,
        );

        assert!(result.is_ok());
        assert_eq!(parser.continuity_issue_count(), 1);
    }

    #[test]
    fn continuity_strict_mode_fails_on_discontinuity() {
        let packet_1 = build_ts_packet(0x0100, false, 0, &[0x00]);
        let packet_2 = build_ts_packet(0x0100, false, 5, &[0x01]);

        let mut stream = Vec::new();
        stream.extend_from_slice(&packet_1);
        stream.extend_from_slice(&packet_2);

        let mut parser = TsParser::new().with_continuity_mode(ContinuityMode::Strict);
        let result = parser.parse_packets(
            Bytes::from(stream),
            |_pat| Ok(()),
            |_pmt| Ok(()),
            None::<fn(&TsPacketRef) -> Result<()>>,
        );

        assert!(matches!(
            result,
            Err(TsError::ContinuityError {
                pid: 0x0100,
                expected: 1,
                actual: 5
            })
        ));
    }

    #[test]
    fn parses_192_byte_m2ts_packets() {
        let pat_section = build_pat_section(0, 1, 0x0100);
        let mut payload = Vec::with_capacity(184);
        payload.push(0x00);
        payload.extend_from_slice(&pat_section);

        let mut packet = vec![0u8; 192];
        packet[0..4].copy_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let ts_packet = build_ts_packet(0x0000, true, 0, &payload);
        packet[4..].copy_from_slice(&ts_packet);

        let mut parser = TsParser::new();
        let mut pat_count = 0usize;

        parser
            .parse_packets(
                Bytes::from(packet),
                |_pat| {
                    pat_count += 1;
                    Ok(())
                },
                |_pmt| Ok(()),
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(pat_count, 1);
    }

    #[test]
    fn parses_204_byte_ts_packets() {
        let pat_section = build_pat_section(0, 1, 0x0100);
        let mut payload = Vec::with_capacity(184);
        payload.push(0x00);
        payload.extend_from_slice(&pat_section);

        let mut packet = vec![0u8; 204];
        let ts_packet = build_ts_packet(0x0000, true, 0, &payload);
        packet[..188].copy_from_slice(&ts_packet);
        packet[188..].copy_from_slice(&[0xAA; 16]);

        let mut parser = TsParser::new();
        let mut pat_count = 0usize;

        parser
            .parse_packets(
                Bytes::from(packet),
                |_pat| {
                    pat_count += 1;
                    Ok(())
                },
                |_pmt| Ok(()),
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(pat_count, 1);
    }

    #[test]
    fn resyncs_after_locked_format_mismatch() {
        let pat_v0 = build_pat_section(0, 1, 0x0100);
        let pat_v1 = build_pat_section(1, 1, 0x0100);

        let mut payload_1 = Vec::with_capacity(184);
        payload_1.push(0x00);
        payload_1.extend_from_slice(&pat_v0);

        let mut payload_2 = Vec::with_capacity(184);
        payload_2.push(0x00);
        payload_2.extend_from_slice(&pat_v1);

        let ts_packet_1 = build_ts_packet(0x0000, true, 0, &payload_1);
        let ts_packet_2 = build_ts_packet(0x0000, true, 1, &payload_2);

        let mut m2ts_packet_1 = vec![0x00, 0x00, 0x00, 0x00];
        m2ts_packet_1.extend_from_slice(&ts_packet_1);

        let mut m2ts_packet_2 = vec![0x00, 0x00, 0x00, 0x00];
        m2ts_packet_2.extend_from_slice(&ts_packet_2);

        let mut stream = Vec::new();
        stream.extend_from_slice(&m2ts_packet_1);
        // Include a decoy sync byte and random data between packets.
        stream.extend_from_slice(&[0x47, 0x99, 0x88, 0x77, 0x66, 0x55]);
        stream.extend_from_slice(&m2ts_packet_2);

        let mut parser = TsParser::new();
        let mut versions = Vec::new();

        parser
            .parse_packets(
                Bytes::from(stream),
                |pat| {
                    versions.push(pat.version_number);
                    Ok(())
                },
                |_pmt| Ok(()),
                None::<fn(&TsPacketRef) -> Result<()>>,
            )
            .unwrap();

        assert_eq!(versions, vec![0, 1]);
    }
}
