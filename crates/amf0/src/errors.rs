use std::io;

use super::define::Amf0Marker;

/// Errors that can occur when decoding AMF0 data.
#[derive(Debug, thiserror::Error)]
pub enum Amf0ReadError {
    /// An unknown marker was encountered.
    #[error("unknown marker: {0}")]
    UnknownMarker(u8),
    /// An unsupported type was encountered.
    #[error("unsupported type: {0:?}")]
    UnsupportedType(Amf0Marker),
    /// A string parse error occurred.
    #[error("string parse error: {0}")]
    StringParseError(#[from] std::str::Utf8Error),
    /// An IO error occurred.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// A wrong type was encountered. Created when using
    /// `Amf0Decoder::next_with_type` and the next value is not the expected
    /// type.
    #[error("wrong type: expected {expected:?}, got {got:?}")]
    WrongType {
        /// The expected type.
        expected: Amf0Marker,
        /// The actual type.
        got: Amf0Marker,
    },
}

impl Amf0ReadError {
    /// Whether this error can be recovered from by skipping bytes.
    ///
    /// Recoverable errors are those where the decoder position is known
    /// and we can safely skip forward to try finding the next valid value.
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::UnknownMarker(_) | Self::UnsupportedType(_))
    }
}

/// Errors that can occur when encoding AMF0 data.
#[derive(Debug, thiserror::Error)]
pub enum Amf0WriteError {
    /// A normal string was too long.
    #[error("normal string too long")]
    NormalStringTooLong,
    /// An IO error occurred.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// An unsupported type was encountered.
    #[error("unsupported type: {0:?}")]
    UnsupportedType(Amf0Marker),
}
