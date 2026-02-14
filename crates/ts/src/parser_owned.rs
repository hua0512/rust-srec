use crate::{
    error::TsError,
    packet::{ContinuityMode, ContinuityStatus, PID_NULL, PID_PAT, TsPacket},
    pat::Pat,
    pmt::Pmt,
};
use bytes::{Buf, Bytes};
use memchr::memchr;
use std::collections::HashMap;

/// Transport Stream parser for PAT and PMT tables
#[derive(Debug, Default)]
pub struct OwnedTsParser {
    /// Cached PAT table
    pat: Option<Pat>,
    /// Cached PMT tables by program number
    pmts: HashMap<u16, Pmt>,
    /// Buffer for incomplete PSI sections
    psi_buffers: HashMap<u16, Vec<u8>>,
    /// Current version numbers to detect updates
    pat_version: Option<u8>,
    pmt_versions: HashMap<u16, u8>, // program_number -> version
    /// Whether to validate CRC-32/MPEG-2 on PAT/PMT sections
    validate_crc: bool,
    /// Continuity counter tracking per PID: pid -> last_cc
    continuity_counters: HashMap<u16, u8>,
    continuity_mode: ContinuityMode,
    continuity_issue_count: usize,
    continuity_duplicate_count: usize,
    continuity_discontinuity_count: usize,
}

impl OwnedTsParser {
    /// Create a new TS parser
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable CRC-32/MPEG-2 validation on PAT/PMT sections.
    pub fn with_crc_validation(mut self, enable: bool) -> Self {
        self.validate_crc = enable;
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

    pub fn with_continuity_mode(mut self, mode: ContinuityMode) -> Self {
        self.continuity_mode = mode;
        self
    }

    pub fn continuity_issue_count(&self) -> usize {
        self.continuity_issue_count
    }

    pub fn continuity_duplicate_count(&self) -> usize {
        self.continuity_duplicate_count
    }

    pub fn continuity_discontinuity_count(&self) -> usize {
        self.continuity_discontinuity_count
    }

    fn handle_continuity_status(
        &mut self,
        pid: u16,
        status: ContinuityStatus,
    ) -> Result<(), TsError> {
        match status {
            ContinuityStatus::Initial | ContinuityStatus::Ok => Ok(()),
            ContinuityStatus::Duplicate => {
                self.continuity_issue_count += 1;
                self.continuity_duplicate_count += 1;
                match self.continuity_mode {
                    ContinuityMode::Disabled | ContinuityMode::Warn => Ok(()),
                    ContinuityMode::Strict => Err(TsError::DuplicatePacket {
                        pid,
                        cc: self.continuity_counters.get(&pid).copied().unwrap_or(0),
                    }),
                }
            }
            ContinuityStatus::Discontinuity { expected, actual } => {
                self.continuity_issue_count += 1;
                self.continuity_discontinuity_count += 1;
                match self.continuity_mode {
                    ContinuityMode::Disabled | ContinuityMode::Warn => Ok(()),
                    ContinuityMode::Strict => Err(TsError::ContinuityError {
                        pid,
                        expected,
                        actual,
                    }),
                }
            }
        }
    }

    fn check_cc(&mut self, packet: &TsPacket) -> ContinuityStatus {
        if packet.pid == PID_NULL {
            return ContinuityStatus::Ok;
        }

        let has_payload = packet.has_payload();
        if let Some(&last_cc) = self.continuity_counters.get(&packet.pid) {
            if has_payload {
                let expected = (last_cc + 1) & 0x0F;
                if packet.continuity_counter == expected {
                    self.continuity_counters
                        .insert(packet.pid, packet.continuity_counter);
                    ContinuityStatus::Ok
                } else if packet.continuity_counter == last_cc {
                    ContinuityStatus::Duplicate
                } else {
                    self.continuity_counters
                        .insert(packet.pid, packet.continuity_counter);
                    ContinuityStatus::Discontinuity {
                        expected,
                        actual: packet.continuity_counter,
                    }
                }
            } else if packet.continuity_counter == last_cc {
                ContinuityStatus::Ok
            } else {
                self.continuity_counters
                    .insert(packet.pid, packet.continuity_counter);
                ContinuityStatus::Discontinuity {
                    expected: last_cc,
                    actual: packet.continuity_counter,
                }
            }
        } else {
            self.continuity_counters
                .insert(packet.pid, packet.continuity_counter);
            ContinuityStatus::Initial
        }
    }

    /// Parse TS packets from bytes and extract PAT/PMT information
    pub fn parse_packets(&mut self, data: Bytes) -> Result<(), TsError> {
        let mut remaining_data = data;

        while !remaining_data.is_empty() {
            let sync_offset = match memchr(0x47, &remaining_data) {
                Some(offset) => offset,
                None => break, // No more sync bytes
            };

            remaining_data.advance(sync_offset);

            if remaining_data.len() < 188 {
                break; // Not enough data for a full packet
            }

            // Now remaining_data is 0x47
            let chunk = remaining_data.slice(..188);

            match TsPacket::parse(chunk) {
                Ok(packet) => {
                    if self.continuity_mode != ContinuityMode::Disabled {
                        let status = self.check_cc(&packet);
                        self.handle_continuity_status(packet.pid, status)?;
                    }

                    if packet.payload_unit_start_indicator {
                        self.process_packet(&packet)?;
                    }
                    remaining_data.advance(188);
                }
                Err(_) => {
                    // The packet was invalid despite the sync byte.
                    // Advance one byte to continue searching from the next position.
                    remaining_data.advance(1);
                }
            }
        }

        Ok(())
    }

    /// Process a single TS packet
    fn process_packet(&mut self, packet: &TsPacket) -> Result<(), TsError> {
        if let Some(psi_payload) = packet.get_psi_payload() {
            if psi_payload.is_empty() {
                return Ok(());
            }

            let table_id = psi_payload[0];

            match packet.pid {
                PID_PAT if table_id == 0x00 => {
                    let pat = if self.validate_crc {
                        Pat::parse_with_crc(&psi_payload)?
                    } else {
                        Pat::parse(&psi_payload)?
                    };
                    self.process_pat(pat)?;
                }
                pid if self.is_pmt_pid(pid) && table_id == 0x02 => {
                    self.process_pmt(pid, &psi_payload)?;
                }
                _ => {
                    // Not a PAT or PMT packet we are interested in
                }
            }
        }

        Ok(())
    }

    /// Check if a PID is a PMT PID based on current PAT
    fn is_pmt_pid(&self, pid: u16) -> bool {
        if let Some(pat) = &self.pat {
            pat.programs.iter().any(|prog| prog.pmt_pid == pid)
        } else {
            false
        }
    }

    /// Parse PAT from payload
    fn process_pat(&mut self, pat: Pat) -> Result<(), TsError> {
        let is_new = self.pat_version != Some(pat.version_number);
        if is_new {
            self.pat_version = Some(pat.version_number);
            self.pmts.clear();
            self.pmt_versions.clear();
            self.pat = Some(pat);
        }
        Ok(())
    }

    /// Parse PMT from payload
    fn process_pmt(&mut self, pid: u16, payload: &[u8]) -> Result<(), TsError> {
        if let Some(pat) = &self.pat
            && let Some(program) = pat.programs.iter().find(|p| p.pmt_pid == pid)
        {
            let pmt = if self.validate_crc {
                Pmt::parse_with_crc(payload)?
            } else {
                Pmt::parse(payload)?
            };
            let is_new = self
                .pmt_versions
                .get(&program.program_number)
                .is_none_or(|&v| v != pmt.version_number);

            if is_new {
                self.pmt_versions
                    .insert(program.program_number, pmt.version_number);
                self.pmts.insert(program.program_number, pmt);
            }
        }
        Ok(())
    }

    /// Get the parsed PAT
    pub fn pat(&self) -> Option<&Pat> {
        self.pat.as_ref()
    }

    /// Get all parsed PMTs
    pub fn pmts(&self) -> &HashMap<u16, Pmt> {
        &self.pmts
    }

    /// Get a specific PMT by program number
    pub fn pmt(&self, program_number: u16) -> Option<&Pmt> {
        self.pmts.get(&program_number)
    }

    /// Reset the parser state
    pub fn reset(&mut self) {
        self.pat = None;
        self.pmts.clear();
        self.psi_buffers.clear();
        self.pat_version = None;
        self.pmt_versions.clear();
        self.continuity_counters.clear();
        self.continuity_issue_count = 0;
        self.continuity_duplicate_count = 0;
        self.continuity_discontinuity_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ts_packet(pid: u16, cc: u8, adaptation_field_control: u8) -> Vec<u8> {
        let mut data = vec![0u8; 188];
        data[0] = 0x47;

        data[1] = ((pid >> 8) as u8) & 0x1F;
        data[2] = (pid & 0xFF) as u8;

        data[3] = ((adaptation_field_control & 0x03) << 4) | (cc & 0x0F);

        if adaptation_field_control == 0x02 || adaptation_field_control == 0x03 {
            data[4] = 0;
        }

        data
    }

    #[test]
    fn test_parser_creation() {
        let parser = OwnedTsParser::new();
        assert!(parser.pat().is_none());
        assert!(parser.pmts().is_empty());
    }

    #[test]
    fn test_continuity_warn_mode_counts_discontinuity() {
        let pid = 0x0033;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ts_packet(pid, 0, 0x01));
        bytes.extend_from_slice(&make_ts_packet(pid, 2, 0x01));

        let mut parser = OwnedTsParser::new().with_continuity_mode(ContinuityMode::Warn);
        parser.parse_packets(Bytes::from(bytes)).unwrap();

        assert_eq!(parser.continuity_issue_count(), 1);
        assert_eq!(parser.continuity_discontinuity_count(), 1);
        assert_eq!(parser.continuity_duplicate_count(), 0);
    }

    #[test]
    fn test_continuity_strict_mode_errors_on_discontinuity() {
        let pid = 0x0033;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ts_packet(pid, 0, 0x01));
        bytes.extend_from_slice(&make_ts_packet(pid, 2, 0x01));

        let mut parser = OwnedTsParser::new().with_continuity_mode(ContinuityMode::Strict);
        let err = parser.parse_packets(Bytes::from(bytes)).unwrap_err();
        assert!(matches!(
            err,
            TsError::ContinuityError {
                pid: p,
                expected: 1,
                actual: 2
            } if p == pid
        ));
    }

    #[test]
    fn test_continuity_strict_mode_errors_on_duplicate() {
        let pid = 0x0033;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ts_packet(pid, 0, 0x01));
        bytes.extend_from_slice(&make_ts_packet(pid, 0, 0x01));

        let mut parser = OwnedTsParser::new().with_continuity_mode(ContinuityMode::Strict);
        let err = parser.parse_packets(Bytes::from(bytes)).unwrap_err();
        assert!(matches!(err, TsError::DuplicatePacket { pid: p, cc: 0 } if p == pid));
    }

    #[test]
    fn test_continuity_adaptation_only_requires_constant_cc() {
        let pid = 0x0033;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ts_packet(pid, 5, 0x02));
        bytes.extend_from_slice(&make_ts_packet(pid, 5, 0x02));

        let mut parser = OwnedTsParser::new().with_continuity_mode(ContinuityMode::Strict);
        parser.parse_packets(Bytes::from(bytes)).unwrap();

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ts_packet(pid, 5, 0x02));
        bytes.extend_from_slice(&make_ts_packet(pid, 6, 0x02));

        let mut parser = OwnedTsParser::new().with_continuity_mode(ContinuityMode::Strict);
        let err = parser.parse_packets(Bytes::from(bytes)).unwrap_err();
        assert!(matches!(
            err,
            TsError::ContinuityError {
                pid: p,
                expected: 5,
                actual: 6
            } if p == pid
        ));
    }
}
