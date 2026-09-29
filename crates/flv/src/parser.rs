use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use bytes::BytesMut;
use tracing::{debug, error};

use crate::header::FlvHeader;
use crate::tag::FlvTagType;
use crate::{framing, tag::FlvTag};

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
                        _ => debug!("Unknown tag type: {:?}", tag.tag_type()),
                    }

                    on_tag(&tag, tag_type, tag_position);
                    expected_prev_tag_size = (framing::TAG_HEADER_SIZE + tag.data().len()) as u32;
                    current_position += (framing::TAG_HEADER_SIZE + tag.data().len()) as u64;
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
                _ => error!("Unknown tag type: {:?}", tag.tag_type()),
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
        let mut header_bytes = [0u8; framing::TAG_HEADER_SIZE];
        if let Err(e) = reader.read_exact(&mut header_bytes) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        let header = framing::parse_tag_header_bytes(header_bytes)?;

        // Keep the fixed header on the stack and allocate only the payload.
        // Freezing it directly also avoids sharing a sliced header/data buffer.
        let mut data = BytesMut::zeroed(header.data_size as usize);
        if let Err(e) = reader.read_exact(&mut data) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        let tag_type = header.tag_type;
        let tag = FlvTag::new(
            header.timestamp_ms,
            header.stream_id,
            header.tag_type,
            header.is_filtered,
            data.freeze(),
        );

        Ok(Some((tag, tag_type)))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    struct ShortReads<R>(R);

    impl<R: Read> Read for ShortReads<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(3);
            self.0.read(&mut buf[..len])
        }
    }

    #[test]
    fn payloads_of_different_sizes_survive_short_reads_without_consuming_the_next_tag() {
        for size in [0, 4, 4095, 4096, 16 * 1024] {
            // Filtered audio is opaque to the container parser.
            let payload: Vec<u8> = (0..size).map(|index| index as u8).collect();
            let mut packet = vec![0x28, 0, 0, 0, 0x34, 0x56, 0x78, 0x12, 0, 0, 0];
            packet[1..4].copy_from_slice(&(size as u32).to_be_bytes()[1..]);
            packet.extend_from_slice(&payload);
            // A complete second tag exercises header/payload boundaries.
            packet.extend_from_slice(&[8, 0, 0, 4, 0, 0, 23, 0, 0, 0, 0, 0xaf, 0, 0x12, 0x10]);

            let mut reader = ShortReads(packet.as_slice());
            assert_eq!(
                FlvParser::parse_tag(&mut reader).unwrap(),
                Some((
                    FlvTag::new(
                        0x1234_5678,
                        0,
                        FlvTagType::Audio,
                        true,
                        Bytes::from(payload)
                    ),
                    FlvTagType::Audio,
                )),
                "payload size {size}"
            );
            assert_eq!(
                FlvParser::parse_tag(&mut reader).unwrap(),
                Some((
                    FlvTag::new(
                        23,
                        0,
                        FlvTagType::Audio,
                        false,
                        Bytes::from_static(b"\xaf\0\x12\x10")
                    ),
                    FlvTagType::Audio,
                ))
            );
            assert!(FlvParser::parse_tag(&mut reader).unwrap().is_none());
        }
    }

    #[test]
    fn incomplete_tags_are_ignored_but_other_read_errors_propagate() {
        let packet = [8, 0, 0, 4, 0, 0, 23, 0, 0, 0, 0, 0xaf, 0, 0x12, 0x10];
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::ConnectionReset.into())
            }
        }

        for len in 0..packet.len() {
            let mut truncated = &packet[..len];
            assert!(
                FlvParser::parse_tag(&mut truncated).unwrap().is_none(),
                "length {len}"
            );

            let mut failed = packet[..len].chain(BrokenReader);
            assert_eq!(
                FlvParser::parse_tag(&mut failed).unwrap_err().kind(),
                io::ErrorKind::ConnectionReset,
                "length {len}"
            );
        }
    }
}
