use bytes::BytesMut;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read};
use std::path::Path;
use tracing::{debug, error};

use crate::header::FlvHeader;
use crate::tag::FlvTagType;
use crate::{framing, tag::FlvTag};

const BUFFER_SIZE: usize = 4 * 1024; // 4 KB buffer size

/// Parser that works with borrowed data (FlvTag).
pub struct FlvParser;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrevTagSizeMode {
    /// Ignore `PreviousTagSize` values (fastest, most tolerant).
    #[default]
    Ignore,
    /// Log mismatches but continue parsing.
    Warn,
    /// Treat any mismatch as an error.
    Strict,
}

impl FlvParser {
    /// Parse the FLV header from a reader.
    pub fn parse_header<R: Read>(reader: &mut R) -> io::Result<FlvHeader> {
        FlvHeader::parse(reader)
    }

    pub fn parse_tags<R, F>(reader: &mut R, mut on_tag: F, current_position: u64) -> io::Result<u32>
    where
        R: Read,
        F: FnMut(&FlvTag, FlvTagType, u64),
    {
        Self::parse_tags_with_prev_tag_size_mode(
            reader,
            &mut on_tag,
            current_position,
            PrevTagSizeMode::Ignore,
        )
    }

    pub fn parse_tags_with_prev_tag_size_mode<R, F>(
        reader: &mut R,
        on_tag: &mut F,
        mut current_position: u64,
        mode: PrevTagSizeMode,
    ) -> io::Result<u32>
    where
        R: Read,
        F: FnMut(&FlvTag, FlvTagType, u64),
    {
        let mut tags_count = 0;
        let mut video_tags = 0;
        let mut audio_tags = 0;
        let mut metadata_tags = 0;

        let mut expected_prev_tag_size = 0u32;

        loop {
            // Read PreviousTagSize (4 bytes).
            let mut prev_tag_buffer = [0u8; framing::PREV_TAG_SIZE_FIELD_SIZE];
            match reader.read_exact(&mut prev_tag_buffer) {
                Ok(_) => {
                    let prev_tag_size = u32::from_be_bytes(prev_tag_buffer);
                    if mode != PrevTagSizeMode::Ignore && prev_tag_size != expected_prev_tag_size {
                        match mode {
                            PrevTagSizeMode::Ignore => {}
                            PrevTagSizeMode::Warn => {
                                debug!(
                                    expected = expected_prev_tag_size,
                                    got = prev_tag_size,
                                    position = current_position,
                                    "PreviousTagSize mismatch"
                                );
                            }
                            PrevTagSizeMode::Strict => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!(
                                        "PreviousTagSize mismatch (expected {expected_prev_tag_size}, got {prev_tag_size})"
                                    ),
                                ));
                            }
                        }
                    }
                    current_position += framing::PREV_TAG_SIZE_FIELD_SIZE as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }

            let tag_position = current_position;

            match Self::parse_tag(reader) {
                Ok(Some((tag, tag_type))) => {
                    tags_count += 1;
                    match tag_type {
                        FlvTagType::Video => video_tags += 1,
                        FlvTagType::Audio => audio_tags += 1,
                        FlvTagType::ScriptData => metadata_tags += 1,
                        _ => debug!("Unknown tag type: {:?}", tag.tag_type),
                    }

                    on_tag(&tag, tag_type, tag_position);
                    expected_prev_tag_size = (framing::TAG_HEADER_SIZE + tag.data.len()) as u32;
                    current_position += (framing::TAG_HEADER_SIZE + tag.data.len()) as u64;
                }
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }

        debug!(
            "Audio tags: {audio_tags}, Video tags: {video_tags}, Metadata tags: {metadata_tags}"
        );

        Ok(tags_count)
    }

    pub fn parse_file(file_path: &Path) -> io::Result<u32> {
        let file = File::open(file_path)?;
        let mut reader = BufReader::new(file);

        // Parse the header
        let header = Self::parse_header(&mut reader)?;
        let initial_position = header.data_offset as u64;

        // Add these variables to track tag types
        let mut video_tags = 0;
        let mut audio_tags = 0;
        let mut metadata_tags = 0;

        let tags_count = Self::parse_tags_with_prev_tag_size_mode(
            &mut reader,
            &mut |tag, tag_type, _pos| match tag_type {
                FlvTagType::Video => video_tags += 1,
                FlvTagType::Audio => audio_tags += 1,
                FlvTagType::ScriptData => metadata_tags += 1,
                _ => error!("Unknown tag type: {:?}", tag.tag_type),
            },
            initial_position,
            PrevTagSizeMode::Ignore,
        )?;

        debug!(
            "Audio tags: {}, Video tags: {}, Metadata tags: {}",
            audio_tags, video_tags, metadata_tags
        );

        Ok(tags_count)
    }

    /// Parse a single FLV tag from a reader
    /// Returns the parsed tag and its type if successful
    /// Returns None if EOF is reached
    pub fn parse_tag<R: Read>(reader: &mut R) -> io::Result<Option<(FlvTag, FlvTagType)>> {
        let mut tag_buffer = BytesMut::with_capacity(BUFFER_SIZE);

        // Peek at tag header (first 11 bytes) to get the data size
        tag_buffer.resize(framing::TAG_HEADER_SIZE, 0);
        if let Err(e) = reader.read_exact(&mut tag_buffer) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        let header = {
            let mut header_bytes = [0u8; framing::TAG_HEADER_SIZE];
            header_bytes.copy_from_slice(&tag_buffer[..framing::TAG_HEADER_SIZE]);
            framing::parse_tag_header_bytes(header_bytes)?
        };

        // Now read the complete tag (header + data)
        // Reset position to beginning of tag
        let total_tag_size = framing::TAG_HEADER_SIZE + header.data_size as usize;

        // Resize the buffer to fit the entire tag
        // We've already read the first 11 bytes, so we need to allocate more space for the data
        tag_buffer.resize(total_tag_size, 0);

        // Read the remaining data (we already have the first 11 bytes)
        if let Err(e) = reader.read_exact(&mut tag_buffer[framing::TAG_HEADER_SIZE..]) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Determine the tag type
        let tag_type = header.tag_type;

        // Demux the tag
        let tag = FlvTag::demux(&mut Cursor::new(tag_buffer.freeze()))?;

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
        let tags_count = super::FlvParser::parse_file(path)?;
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
