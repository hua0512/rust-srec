use byteorder::{BigEndian, ReadBytesExt};
use bytes::{Buf, BytesMut};
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read};
use std::path::Path;
use tracing::{debug, error};

use crate::header::FlvHeader;
use crate::tag::{FlvTag, FlvTagData, FlvTagOwned, FlvTagType, FlvUtil};

const BUFFER_SIZE: usize = 4 * 1024; // 4 KB buffer size

/// Parser that works with owned data (FlvTagOwned)
pub struct FlvParser;

/// Parser that works with borrowed data (FlvTag)
pub struct FlvParserRef;

impl FlvParser {
    pub fn parse_file(file_path: &Path) -> io::Result<u32> {
        let file = File::open(file_path)?;
        let mut reader = BufReader::new(file);

        // Parse the header
        let _header = Self::parse_header(&mut reader)?;
        let mut tags_count = 0;

        // Add these variables to track tag types
        let mut video_tags = 0;
        let mut audio_tags = 0;
        let mut metadata_tags = 0;

        loop {
            // Read previous tag size (4 bytes)
            let mut prev_tag_buffer = BytesMut::with_capacity(4);
            prev_tag_buffer.resize(4, 0);
            match reader.read_exact(&mut prev_tag_buffer) {
                Ok(_) => {
                    // Just ignore the prev tag size for now
                    let _prev_tag_size = (&prev_tag_buffer[..]).get_u32();
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(e),
            }

            // Parse a single tag
            match Self::parse_tag(&mut reader) {
                Ok(Some((tag, tag_type))) => {
                    tags_count += 1;
                    match tag_type {
                        FlvTagType::Video => video_tags += 1,
                        FlvTagType::Audio => audio_tags += 1,
                        FlvTagType::ScriptData => metadata_tags += 1,
                        _ => error!("Unknown tag type: {:?}", tag),
                    }
                }
                Ok(None) => break, // End of file
                Err(e) => return Err(e),
            }
        }

        debug!(
            "Audio tags: {}, Video tags: {}, Metadata tags: {}",
            audio_tags, video_tags, metadata_tags
        );

        Ok(tags_count)
    }

    /// Parse the FLV header from a reader
    fn parse_header<R: Read>(reader: &mut R) -> io::Result<FlvHeader> {
        let mut buffer = BytesMut::with_capacity(9);
        buffer.resize(9, 0);
        reader.read_exact(&mut buffer)?;
        let mut cursor = Cursor::new(buffer.freeze());
        FlvHeader::parse(&mut cursor)
    }

    /// Parse a single FLV tag from a reader
    /// Returns the parsed tag and its type if successful
    /// Returns None if EOF is reached
    pub fn parse_tag<R: Read>(reader: &mut R) -> io::Result<Option<(FlvTagOwned, FlvTagType)>> {
        let mut tag_buffer = BytesMut::with_capacity(BUFFER_SIZE);

        // Peek at tag header (first 11 bytes) to get the data size
        tag_buffer.resize(11, 0);
        if let Err(e) = reader.read_exact(&mut tag_buffer) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Get the data size from the first 11 bytes without consuming the buffer
        let data_size = {
            let mut peek = Cursor::new(&tag_buffer[..]);
            peek.advance(1); // Skip tag type byte
            peek.read_u24::<BigEndian>()?
        };

        // Now read the complete tag (header + data)
        // Reset position to beginning of tag
        let total_tag_size = 11 + data_size as usize;

        // Resize the buffer to fit the entire tag
        // We've already read the first 11 bytes, so we need to allocate more space for the data
        tag_buffer.resize(total_tag_size, 0);

        // Read the remaining data (we already have the first 11 bytes)
        if let Err(e) = reader.read_exact(&mut tag_buffer[11..]) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Use FlvTag::demux to parse the entire tag
        let tag = FlvTagOwned::demux(&mut Cursor::new(tag_buffer.freeze()))?;

        // Determine the tag type
        let tag_type = match tag.data {
            FlvTagData::Video(_) => FlvTagType::Video,
            FlvTagData::Audio(_) => FlvTagType::Audio,
            FlvTagData::ScriptData(_) => FlvTagType::ScriptData,
            FlvTagData::Unknown { tag_type, data: _ } => FlvTagType::Unknown(tag_type.into()),
        };

        Ok(Some((tag, tag_type)))
    }
}

/// Implementation of the non-owned version of FlvParser
impl FlvParserRef {
    pub fn parse_file(file_path: &Path) -> io::Result<u32> {
        let file = File::open(file_path)?;
        let mut reader = BufReader::new(file);

        // Parse the header
        let _header = Self::parse_header(&mut reader)?;
        let mut tags_count = 0;

        // Add these variables to track tag types
        let mut video_tags = 0;
        let mut audio_tags = 0;
        let mut metadata_tags = 0;

        loop {
            // Read previous tag size (4 bytes)
            let mut prev_tag_buffer = BytesMut::with_capacity(4);
            prev_tag_buffer.resize(4, 0);
            match reader.read_exact(&mut prev_tag_buffer) {
                Ok(_) => {
                    // Just ignore the prev tag size for now
                    let _prev_tag_size = (&prev_tag_buffer[..]).get_u32();
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(e),
            }

            // Parse a single tag
            match Self::parse_tag(&mut reader) {
                Ok(Some((tag, tag_type))) => {
                    tags_count += 1;
                    match tag_type {
                        FlvTagType::Video => video_tags += 1,
                        FlvTagType::Audio => audio_tags += 1,
                        FlvTagType::ScriptData => metadata_tags += 1,
                        _ => println!("Unknown tag type: {:?}", tag.tag_type),
                    }
                }
                Ok(None) => break, // End of file
                Err(e) => return Err(e),
            }
        }

        debug!(
            "Audio tags: {audio_tags}, Video tags: {video_tags}, Metadata tags: {metadata_tags}"
        );

        Ok(tags_count)
    }

    /// Parse the FLV header from a reader
    fn parse_header<R: Read>(reader: &mut R) -> io::Result<FlvHeader> {
        let mut buffer = BytesMut::with_capacity(9);
        buffer.resize(9, 0);
        reader.read_exact(&mut buffer)?;
        let mut cursor = Cursor::new(buffer.freeze());
        FlvHeader::parse(&mut cursor)
    }

    /// Parse a single FLV tag from a reader
    /// Returns the parsed tag and its type if successful
    /// Returns None if EOF is reached
    pub fn parse_tag<R: Read>(reader: &mut R) -> io::Result<Option<(FlvTag, FlvTagType)>> {
        let mut tag_buffer = BytesMut::with_capacity(BUFFER_SIZE);

        // Peek at tag header (first 11 bytes) to get the data size
        tag_buffer.resize(11, 0);
        if let Err(e) = reader.read_exact(&mut tag_buffer) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Get the data size from the first 11 bytes without consuming the buffer
        let data_size = {
            let mut peek = Cursor::new(&tag_buffer[..]);
            peek.advance(1); // Skip tag type byte
            peek.read_u24::<BigEndian>()?
        };

        // Now read the complete tag (header + data)
        // Reset position to beginning of tag
        let total_tag_size = 11 + data_size as usize;

        // Resize the buffer to fit the entire tag
        // We've already read the first 11 bytes, so we need to allocate more space for the data
        tag_buffer.resize(total_tag_size, 0);

        // Read the remaining data (we already have the first 11 bytes)
        if let Err(e) = reader.read_exact(&mut tag_buffer[11..]) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Determine the tag type
        // peek the first 1 byte to get the tag type
        let tag_type = tag_buffer[0] & 0x1F; // Mask to get the tag type (5 bits)

        // Use FlvTag::demux to parse the entire tag
        let tag = FlvTag::demux(&mut Cursor::new(tag_buffer.freeze()))?;
        let tag_type = FlvTagType::from(tag_type);

        Ok(Some((tag, tag_type)))
    }
}

mod tests {
    #[tokio::test]
    #[ignore] // Ignore this test for now
    async fn test_read_file() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::path::Path::new("D:/test/999/16_02_26-福州~ 主播恋爱脑！！！.flv");

        // Skip the test if the file doesn't exist
        if !path.exists() {
            println!("Test file not found, skipping test");
            return Ok(());
        }

        // Get file size before parsing
        let file_size = std::fs::metadata(path)?.len();
        let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

        let start = std::time::Instant::now(); // Start timer
        let tags_count = super::FlvParser::parse_file(path)?;
        let duration = start.elapsed(); // Stop timer

        // Calculate read speed
        let seconds = duration.as_secs() as f64 + duration.subsec_nanos() as f64 * 1e-9;
        let speed_mbps = file_size_mb / seconds;

        println!("Parsed FLV file in {duration:?}");
        println!("File size: {file_size_mb:.2} MB");
        println!("Read speed: {speed_mbps:.2} MB/s");

        println!("Successfully parsed FLV file with {tags_count} tags");

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore this test for now
    async fn test_read_file_ref() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::path::Path::new("D:/test/999/test.flv");

        // Skip the test if the file doesn't exist
        if !path.exists() {
            println!("Test file not found, skipping test");
            return Ok(());
        }

        // Get file size before parsing
        let file_size = std::fs::metadata(path)?.len();
        let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

        let start = std::time::Instant::now(); // Start timer
        let tags_count = super::FlvParserRef::parse_file(path)?;
        let duration = start.elapsed(); // Stop timer

        // Calculate read speed
        let seconds = duration.as_secs() as f64 + duration.subsec_nanos() as f64 * 1e-9;
        let speed_mbps = file_size_mb / seconds;

        println!("Parsed FLV file (RefParser) in {duration:?}");
        println!("File size: {file_size_mb:.2} MB");
        println!("Read speed: {speed_mbps:.2} MB/s");

        println!("Successfully parsed FLV file with {tags_count} tags");

        Ok(())
    }
}
