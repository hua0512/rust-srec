//! Transport Stream (TS) parser for MPEG-2 Transport Stream data
//!
//! This crate provides functionality to parse Program Association Table (PAT),
//! Program Map Table (PMT), PES headers, adaptation fields, descriptors,
//! and SCTE-35 splice information from MPEG-TS (Transport Stream) data.

pub mod adaptation_field;
pub mod crc32;
pub mod descriptor;
pub mod error;
pub mod packet;
pub mod parser_owned;
pub mod parser_zero_copy;
pub mod pat;
pub mod pes;
pub mod pmt;
pub mod scte35;

pub use adaptation_field::{AdaptationField, AdaptationFieldRef, Pcr};
pub use crc32::{mpeg2_crc32, validate_section_crc32};
pub use descriptor::{Ac3Descriptor, DescriptorIterator, DescriptorRef, LanguageEntry};
pub use error::TsError;
pub use packet::{ContinuityMode, ContinuityStatus, PID_CAT, PID_NULL, PID_PAT, TsPacket};
pub use parser_owned::OwnedTsParser;
pub use parser_zero_copy::{
    PatProgramIterator, PatProgramRef, PatRef, PmtRef, PmtStreamIterator, PmtStreamRef,
    TsPacketRef, TsParser,
};
pub use pat::{Pat, PatProgram};
pub use pes::{PesHeader, PesHeaderRef};
pub use pmt::{Pmt, PmtStream, StreamType};
pub use scte35::{
    BreakDuration, SpliceCommand, SpliceCommandType, SpliceInfoSection, SpliceInfoSectionRef,
    SpliceInsert, TimeSignal,
};

/// Result type for TS parsing operations
pub type Result<T> = std::result::Result<T, TsError>;
