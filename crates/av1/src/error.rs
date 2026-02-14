//! Error types for AV1 container format operations.

use thiserror::Error;

/// Errors that can occur during AV1 container parsing and writing.
#[derive(Error, Debug)]
pub enum Av1Error {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid IVF file signature (expected `"DKIF"`).
    #[error("invalid IVF signature: expected \"DKIF\", got {0:?}")]
    InvalidIvfSignature([u8; 4]),

    /// Invalid IVF codec FourCC (expected `"AV01"` or `"av01"`).
    #[error("invalid IVF codec: expected \"AV01\" or \"av01\", got {0:?}")]
    InvalidIvfCodec([u8; 4]),

    /// Unsupported IVF version.
    #[error("unsupported IVF version: {0}")]
    UnsupportedIvfVersion(u16),

    /// Invalid IVF timebase (zero numerator or denominator).
    #[error("invalid IVF timebase: {numerator}/{denominator}")]
    InvalidIvfTimebase {
        /// Timebase numerator.
        numerator: u32,
        /// Timebase denominator.
        denominator: u32,
    },

    /// Invalid OBU data.
    #[error("invalid OBU: {0}")]
    InvalidObu(String),

    /// LEB128 value overflow.
    #[error("LEB128 overflow: value exceeds maximum")]
    Leb128Overflow,

    /// Unexpected end of data.
    #[error("unexpected end of data: expected {expected} bytes, got {actual}")]
    UnexpectedEof {
        /// Expected number of bytes.
        expected: usize,
        /// Actual number of bytes available.
        actual: usize,
    },

    /// Annex B frame unit size mismatch.
    #[error("Annex B frame unit size mismatch: declared {declared}, consumed {consumed}")]
    FrameUnitSizeMismatch {
        /// Size declared in the frame unit length prefix.
        declared: u64,
        /// Actual number of bytes consumed.
        consumed: u64,
    },

    /// Annex B temporal unit size mismatch.
    #[error("Annex B temporal unit size mismatch: declared {declared}, consumed {consumed}")]
    TemporalUnitSizeMismatch {
        /// Size declared in the temporal unit length prefix.
        declared: u64,
        /// Actual number of bytes consumed.
        consumed: u64,
    },
}

/// Result type alias for AV1 container operations.
pub type Result<T> = std::result::Result<T, Av1Error>;
