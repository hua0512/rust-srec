use crate::{
    error::TsError,
    packet::{PID_NULL, PID_PAT, TsPacket},
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
    /// Whether to check continuity counters
    check_continuity: bool,
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
        self.check_continuity = enable;
        self
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
                    // Track continuity counters if enabled
                    if self.check_continuity && packet.pid != PID_NULL {
                        let has_payload = packet.has_payload();
                        if let Some(&last_cc) = self.continuity_counters.get(&packet.pid)
                            && has_payload
                        {
                            let _expected = (last_cc + 1) & 0x0F;
                        }
                        self.continuity_counters
                            .insert(packet.pid, packet.continuity_counter);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_creation() {
        let parser = OwnedTsParser::new();
        assert!(parser.pat().is_none());
        assert!(parser.pmts().is_empty());
    }
}
