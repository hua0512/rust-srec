use thiserror::Error;

/// Errors that can occur during TS parsing
#[derive(Error, Debug)]
pub enum TsError {
    #[error("Invalid packet size: expected multiple of 188 bytes, got {0}")]
    InvalidPacketSize(usize),

    #[error("Invalid sync byte: expected 0x47, got 0x{0:02x}")]
    InvalidSyncByte(u8),

    #[error("Insufficient data: expected at least {expected} bytes, got {actual}")]
    InsufficientData { expected: usize, actual: usize },

    #[error("Invalid table ID: expected {expected}, got {actual}")]
    InvalidTableId { expected: u8, actual: u8 },

    #[error("Invalid section length: {0}")]
    InvalidSectionLength(u16),

    #[error("CRC32 mismatch: expected 0x{expected:08x}, calculated 0x{calculated:08x}")]
    Crc32Mismatch { expected: u32, calculated: u32 },

    #[error("Invalid program number: {0}")]
    InvalidProgramNumber(u16),

    #[error("Invalid PID: {0}")]
    InvalidPid(u16),

    #[error("Parse error: {0}")]
    ParseError(String),
} 