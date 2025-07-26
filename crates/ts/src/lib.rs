//! Transport Stream (TS) parser for PAT and PMT tables
//!
//! This crate provides functionality to parse Program Association Table (PAT)
//! and Program Map Table (PMT) from MPEG-TS (Transport Stream) data.

pub mod error;
pub mod packet;
pub mod parser_owned;
pub mod parser_zero_copy;
pub mod pat;
pub mod pmt;

pub use error::TsError;
pub use packet::{PID_NULL, PID_PAT, TsPacket};
pub use parser_owned::OwnedTsParser;
pub use parser_zero_copy::{
    PatProgramIterator, PatProgramRef, PatRef, PmtRef, PmtStreamIterator, PmtStreamRef,
    TsPacketRef, TsParser,
};
pub use pat::{Pat, PatProgram};
pub use pmt::{Pmt, PmtStream, StreamType};

/// Result type for TS parsing operations
pub type Result<T> = std::result::Result<T, TsError>;
