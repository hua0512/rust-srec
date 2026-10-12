//! Four-character box type codes (ISO/IEC 14496-12 §4.2).

use std::fmt;

/// A box type such as `moof` or `trak`.
///
/// `Display` renders printable ASCII as-is and escapes anything else, so
/// values taken from untrusted input are safe to log.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    pub const fn new(code: &[u8; 4]) -> Self {
        Self(*code)
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in &self.0 {
            if byte.is_ascii_graphic() || byte == b' ' {
                write!(f, "{}", byte as char)?;
            } else {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCc({self})")
    }
}

impl From<[u8; 4]> for FourCc {
    fn from(code: [u8; 4]) -> Self {
        Self(code)
    }
}

impl From<&[u8; 4]> for FourCc {
    fn from(code: &[u8; 4]) -> Self {
        Self(*code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_escapes_non_printable_bytes() {
        assert_eq!(FourCc::new(b"moof").to_string(), "moof");
        assert_eq!(FourCc::new(b"ac-3").to_string(), "ac-3");
        assert_eq!(FourCc([0, b'a', 0xff, b' ']).to_string(), "\\x00a\\xff ");
    }
}
