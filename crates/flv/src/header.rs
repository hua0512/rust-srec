use std::fmt::Display;
use std::io;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;

const FLV_HEADER_SIZE: usize = 9;

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
    pub data_offset: Bytes,
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
            signature_string,
            self.version,
            self.has_audio,
            self.has_video,
            self.data_offset.len()
        )
    }
}

// Define a trait for readers that can provide the necessary data for FlvHeader parsing
pub trait FlvHeaderReader {
    fn read_signature(&mut self) -> io::Result<u32>;
    fn read_version(&mut self) -> io::Result<u8>;
    fn read_flags(&mut self) -> io::Result<u8>;
    fn read_data_offset(&mut self) -> io::Result<usize>;
    fn get_header_bytes(&self, offset: usize) -> Bytes;
    fn get_position(&self) -> usize;
    fn set_position(&mut self, pos: usize);
}

// Implement for Cursor<Bytes> (existing implementation)
impl FlvHeaderReader for io::Cursor<Bytes> {
    fn read_signature(&mut self) -> io::Result<u32> {
        self.read_u24::<BigEndian>()
    }

    fn read_version(&mut self) -> io::Result<u8> {
        self.read_u8()
    }

    fn read_flags(&mut self) -> io::Result<u8> {
        self.read_u8()
    }

    fn read_data_offset(&mut self) -> io::Result<usize> {
        Ok(self.read_u32::<BigEndian>()? as usize)
    }

    fn get_header_bytes(&self, offset: usize) -> Bytes {
        let end = self.position() as usize;
        self.get_ref().slice(offset..end)
    }

    fn get_position(&self) -> usize {
        self.position() as usize
    }

    fn set_position(&mut self, pos: usize) {
        self.set_position(pos as u64);
    }
}

// Implement for Cursor<&[u8]> (new implementation)
impl FlvHeaderReader for io::Cursor<&[u8]> {
    fn read_signature(&mut self) -> io::Result<u32> {
        self.read_u24::<BigEndian>()
    }

    fn read_version(&mut self) -> io::Result<u8> {
        self.read_u8()
    }

    fn read_flags(&mut self) -> io::Result<u8> {
        self.read_u8()
    }

    fn read_data_offset(&mut self) -> io::Result<usize> {
        Ok(self.read_u32::<BigEndian>()? as usize)
    }

    fn get_header_bytes(&self, offset: usize) -> Bytes {
        let end = self.position() as usize;
        Bytes::copy_from_slice(&self.get_ref()[offset..end])
    }

    fn get_position(&self) -> usize {
        self.position() as usize
    }

    fn set_position(&mut self, pos: usize) {
        self.set_position(pos as u64);
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
            data_offset: Bytes::new(),
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
    /// - `reader`: A mutable reference to a reader implementing the FlvHeaderReader trait.
    ///   The reader will be advanced to the end of the header.
    pub fn parse<R: FlvHeaderReader>(reader: &mut R) -> io::Result<Self> {
        let start = reader.get_position();

        // Signature is a 3-byte string 'FLV'
        let signature = reader.read_signature()?;

        // compare if signature is 'FLV'
        if signature != 0x464C56 {
            // move the cursor back to the start position
            reader.set_position(start);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid FLV signature",
            ));
        }

        // Version is a 1-byte value
        let version = reader.read_version()?;
        // Flags is a 1-byte value
        let flags = reader.read_flags()?;
        // Has audio and video flags
        let has_audio = flags & 0b00000100 != 0;
        let has_video = flags & 0b00000001 != 0;

        // Data offset is a 4-byte value
        let data_offset = reader.read_data_offset()?;

        let end = reader.get_position();
        // Check if the data offset is valid
        let size = end - start;

        if size < FLV_HEADER_SIZE || data_offset != FLV_HEADER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid FLV header size: {size}, {data_offset}"),
            ));
        }

        // Get the bytes for data_offset
        let offset = reader.get_header_bytes(data_offset);

        Ok(FlvHeader {
            signature,
            version,
            has_audio,
            has_video,
            data_offset: offset,
        })
    }

    /// Legacy compatibility method that specifically works with Cursor<Bytes>
    #[deprecated(since = "0.2.0", note = "Use the generic parse method instead")]
    pub fn parse_bytes(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        Self::parse(reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{BigEndian, WriteBytesExt};
    use bytes::Bytes;
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
        let bytes = Bytes::from(buffer.clone());
        let mut reader = Cursor::new(bytes);

        // Parse the header
        let header = FlvHeader::parse(&mut reader).unwrap();

        // Verify the parsed values
        assert_eq!(header.signature, 0x464C56); // "FLV" in hex
        assert_eq!(header.version, 0x01);
        assert!(header.has_audio);
        assert!(header.has_video);
        assert_eq!(reader.position(), 9); // Reader should be at position 9

        // Test with slice cursor (new implementation)
        let mut slice_reader = Cursor::new(&buffer[..]);
        let slice_header = FlvHeader::parse(&mut slice_reader).unwrap();

        // Verify the parsed values
        assert_eq!(slice_header.signature, 0x464C56);
        assert_eq!(slice_header.version, 0x01);
        assert!(slice_header.has_audio);
        assert!(slice_header.has_video);
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

        // Verify reader position is reset to start
        assert_eq!(reader.position(), 0);
    }

    // Additional tests remain mostly unchanged
    // ...
}
