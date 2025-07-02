use thiserror::Error;

#[derive(Error, Debug)]
pub enum TarsError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("Invalid UTF-8 sequence")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    #[error("Invalid tag: {0}")]
    InvalidTag(u8),

    #[error("Invalid type ID: {0}")]
    InvalidTypeId(u8),

    #[error("Missing required field with tag {0}")]
    MissingRequiredField(u8),

    #[error("Tag not found: {0}")]
    TagNotFound(u8),

    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        expected: &'static str,
        actual: &'static str,
    },

    #[error("Unknown error")]
    Unknown,
}