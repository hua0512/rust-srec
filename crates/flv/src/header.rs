use std::fmt::Display;
use std::io;
use std::io::Read;

use byteorder::{BigEndian, ReadBytesExt};

const FLV_HEADER_SIZE: usize = 9;
// DataOffset is a 32-bit header length field. In practice it is 9 for standard FLV.
// Put a conservative bound to avoid buffering unbounded data for a bogus header.
const MAX_DATA_OFFSET: u32 = 64 * 1024;

// Struct representing the FLV header, 9 bytes in total
#[derive(Debug, Clone, PartialEq)]
pub struct FlvHeader {
    pub signature: u32, // The signature of the FLV file, 3 bytes, always 'FLV'
    // The version of the FLV file format, 1 byte, usually 0x01
    pub version: u8,
    // Whether the FLV file contains audio data, 1 byte
    pub has_audio: bool,
    // Whether the FLV file contains video data, 1 byte
    pub has_video: bool,
    // Total size of the header, 4 bytes, always 0x09
    pub data_offset: u32,
}

impl Display for FlvHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Convert signature to a string (FLV)
        let signature_string = format!(
            "{}{}{}",
            ((self.signature >> 16) & 0xFF) as u8 as char,
            ((self.signature >> 8) & 0xFF) as u8 as char,
            (self.signature & 0xFF) as u8 as char
        );

        write!(
            f,
            "FLV Header: \n\
            Signature: {}\n\
            Version: {}\n\
            Has Audio: {}\n\
            Has Video: {}\n\
            Data Offset: {}",
            signature_string, self.version, self.has_audio, self.has_video, self.data_offset
        )
    }
}

impl FlvHeader {
    /// Creates a new `FlvHeader` with the specified audio and video flags.
    /// The signature is always set to 'FLV' (0x464C56) and the version is set to 0x01.
    pub fn new(has_audio: bool, has_video: bool) -> Self {
        FlvHeader {
            signature: 0x464C56, // "FLV" in hex
            version: 0x01,
            has_audio,
            has_video,
            data_offset: FLV_HEADER_SIZE as u32,
        }
    }

    /// Parses the FLV header from a byte stream.
    /// Returns a `FlvHeader` struct if successful, or an error if the header is invalid.
    /// The function reads the first 9 bytes of the stream and checks for the FLV signature.
    /// If the signature is not 'FLV', it returns an error.
    /// The function also checks if the data offset is valid and returns an error if it is not.
    ///
    /// This function can return an `io::Error` if buffer is not enough or if the header is invalid.
    /// Arguments:
    /// - `reader`: A reader positioned at the start of an FLV header.
    ///   The reader will be advanced to `data_offset`.
    pub fn parse<R: Read>(reader: &mut R) -> io::Result<Self> {
        // Signature is a 3-byte string 'FLV'
        let signature = reader.read_u24::<BigEndian>()?;

        if signature != 0x464C56 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid FLV signature",
            ));
        }

        // Version is a 1-byte value. Legacy FLV files are version 1.
        let version = reader.read_u8()?;
        if version != 0x01 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported FLV version: {version}"),
            ));
        }

        // Flags is a 1-byte value. Reserved bits MUST be 0.
        let flags = reader.read_u8()?;
        // Reserved bits are: bits 7..=3 and bit 1.
        if (flags & 0b1111_1010) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid FLV header flags (reserved bits set): 0x{flags:02X}"),
            ));
        }

        let has_audio = (flags & 0b0000_0100) != 0;
        let has_video = (flags & 0b0000_0001) != 0;

        // DataOffset is a 4-byte value specifying the length of the header.
        let data_offset = reader.read_u32::<BigEndian>()?;
        if data_offset < (FLV_HEADER_SIZE as u32) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid FLV DataOffset: {data_offset}"),
            ));
        }

        if data_offset > MAX_DATA_OFFSET {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("FLV DataOffset too large: {data_offset}"),
            ));
        }

        // Skip any extra header bytes.
        let extra = (data_offset as usize).saturating_sub(FLV_HEADER_SIZE);
        if extra > 0 {
            let mut limited = reader.take(extra as u64);
            io::copy(&mut limited, &mut io::sink())?;
            if limited.limit() != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Unexpected EOF while skipping extended FLV header bytes",
                ));
            }
        }

        Ok(FlvHeader {
            signature,
            version,
            has_audio,
            has_video,
            data_offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{BigEndian, WriteBytesExt};
    use std::io::Cursor;

    fn create_valid_header_bytes() -> Vec<u8> {
        let mut buffer = Vec::new();
        // Write "FLV" signature (3 bytes)
        buffer.extend_from_slice(b"FLV");
        // Write version (1 byte)
        buffer.push(0x01);
        // Write flags (1 byte - both audio and video)
        buffer.push(0x05);
        // Write data offset (4 bytes - standard 9)
        buffer.write_u32::<BigEndian>(9).unwrap();
        buffer
    }

    #[test]
    fn test_valid_flv_header() {
        // Create a buffer with a valid FLV header
        let buffer = create_valid_header_bytes();

        // Test with Bytes cursor (original implementation)
        let mut reader = Cursor::new(&buffer[..]);

        // Parse the header
        let header = FlvHeader::parse(&mut reader).unwrap();

        // Verify the parsed values
        assert_eq!(header.signature, 0x464C56); // "FLV" in hex
        assert_eq!(header.version, 0x01);
        assert!(header.has_audio);
        assert!(header.has_video);
        assert_eq!(header.data_offset, 9);
        assert_eq!(reader.position(), 9); // Reader should be at position 9

        // Test with slice cursor (new implementation)
        let mut slice_reader = Cursor::new(&buffer[..]);
        let slice_header = FlvHeader::parse(&mut slice_reader).unwrap();

        // Verify the parsed values
        assert_eq!(slice_header.signature, 0x464C56);
        assert_eq!(slice_header.version, 0x01);
        assert!(slice_header.has_audio);
        assert!(slice_header.has_video);
        assert_eq!(slice_header.data_offset, 9);
        assert_eq!(slice_reader.position(), 9);
    }

    #[test]
    fn test_invalid_flv_signature() {
        // Create a buffer with an invalid signature
        let mut buffer = Vec::new();

        // Write invalid signature "ABC" instead of "FLV"
        buffer.extend_from_slice(b"ABC");

        // Add remaining header bytes
        buffer.push(0x01);
        buffer.push(0x03);
        buffer.write_u32::<BigEndian>(9).unwrap();

        // Test with slice cursor
        let mut reader = Cursor::new(&buffer[..]);

        // Parse should fail with invalid signature
        let result = FlvHeader::parse(&mut reader);
        assert!(result.is_err());
    }

    // Additional tests remain mostly unchanged
    // ...
}
