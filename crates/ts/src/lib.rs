//! Transport Stream (TS) parser for PAT and PMT tables
//!
//! This crate provides functionality to parse Program Association Table (PAT)
//! and Program Map Table (PMT) from MPEG-TS (Transport Stream) data.

use std::collections::HashMap;

pub mod error;
pub mod packet;
pub mod pat;
pub mod pmt;
pub mod zero_copy;

pub use error::TsError;
pub use packet::{PID_NULL, PID_PAT, TsPacket};
pub use pat::{Pat, PatProgram};
pub use pmt::{Pmt, PmtStream, StreamType};
pub use zero_copy::{
    PatProgramIterator, PatProgramRef, PatRef, PmtRef, PmtStreamIterator, PmtStreamRef,
    TsPacketRef, ZeroCopyTsParser,
};

/// Result type for TS parsing operations
pub type Result<T> = std::result::Result<T, TsError>;

/// Transport Stream parser for PAT and PMT tables
#[derive(Debug, Default)]
pub struct TsParser {
    /// Cached PAT table
    pat: Option<Pat>,
    /// Cached PMT tables by program number
    pmts: HashMap<u16, Pmt>,
    /// Buffer for incomplete PSI sections
    psi_buffers: HashMap<u16, Vec<u8>>,
}

impl TsParser {
    /// Create a new TS parser
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse TS packets from bytes and extract PAT/PMT information
    pub fn parse_packets(&mut self, data: &[u8]) -> Result<()> {
        if data.len() % 188 != 0 {
            return Err(TsError::InvalidPacketSize(data.len()));
        }

        for chunk in data.chunks_exact(188) {
            let packet = TsPacket::parse(chunk)?;
            self.process_packet(&packet)?;
        }

        Ok(())
    }

    /// Process a single TS packet
    fn process_packet(&mut self, packet: &TsPacket) -> Result<()> {
        match packet.pid {
            PID_PAT => {
                if let Some(psi_payload) = packet.get_psi_payload() {
                    self.parse_pat(&psi_payload)?;
                }
            }
            pid if self.is_pmt_pid(pid) => {
                if let Some(psi_payload) = packet.get_psi_payload() {
                    self.parse_pmt(pid, &psi_payload)?;
                }
            }
            _ => {
                // Ignore other PIDs for now
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
    fn parse_pat(&mut self, payload: &[u8]) -> Result<()> {
        let pat = Pat::parse(payload)?;
        self.pat = Some(pat);
        Ok(())
    }

    /// Parse PMT from payload
    fn parse_pmt(&mut self, pid: u16, payload: &[u8]) -> Result<()> {
        if let Some(pat) = &self.pat {
            if let Some(program) = pat.programs.iter().find(|p| p.pmt_pid == pid) {
                let pmt = Pmt::parse(payload)?;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_creation() {
        let parser = TsParser::new();
        assert!(parser.pat().is_none());
        assert!(parser.pmts().is_empty());
    }

    #[test]
    fn test_invalid_packet_size() {
        let mut parser = TsParser::new();
        let data = vec![0u8; 100]; // Invalid size
        assert!(parser.parse_packets(&data).is_err());
    }
}
