//! FLV Pipe Output Strategy
//!
//! This module provides a strategy for writing FLV data to pipe output (stdout).
//! It implements segment boundary detection to close the pipe when a new FLV header
//! or EndOfSequence marker is received.

use byteorder::{BigEndian, WriteBytesExt};
use flv::data::FlvData;
use flv::header::FlvHeader;
use flv::tag::FlvTag;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

use pipeline_common::{FormatStrategy, PostWriteAction, WriterConfig, WriterState};

/// Error type for pipe FLV strategy
#[derive(Debug, Error)]
pub enum PipeFlvStrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Broken pipe: consumer closed the connection")]
    BrokenPipe,
}

impl PipeFlvStrategyError {
    /// Check if this error is a broken pipe error
    #[allow(dead_code)]
    pub fn is_broken_pipe(&self) -> bool {
        match self {
            PipeFlvStrategyError::BrokenPipe => true,
            PipeFlvStrategyError::Io(e) => e.kind() == io::ErrorKind::BrokenPipe,
        }
    }

    /// Create a broken pipe error from an I/O error if applicable
    pub fn from_io_error(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::BrokenPipe {
            warn!("Broken pipe detected: consumer closed the connection");
            PipeFlvStrategyError::BrokenPipe
        } else {
            PipeFlvStrategyError::Io(err)
        }
    }
}

/// FLV pipe output strategy
///
/// This strategy writes FLV data to a pipe (stdout) and detects segment boundaries
/// to signal when the pipe should be closed. Boundaries are detected when:
/// - A new FLV header is received after data has already been written
/// - An EndOfSequence marker is received
pub struct PipeFlvStrategy {
    /// Whether any data has been written to the current pipe
    has_written_data: bool,
    /// Total bytes written
    bytes_written: u64,
    /// Whether a boundary was detected that should trigger pipe closure
    should_close: bool,
    /// Count of segment boundaries encountered
    segment_count: u32,
}

impl Default for PipeFlvStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeFlvStrategy {
    /// Create a new PipeFlvStrategy
    pub fn new() -> Self {
        Self {
            has_written_data: false,
            bytes_written: 0,
            should_close: false,
            segment_count: 0,
        }
    }

    /// Get the current segment count
    #[allow(dead_code)]
    pub fn segment_count(&self) -> u32 {
        self.segment_count
    }

    /// Check if the pipe should be closed based on the current item
    ///
    /// Returns true when:
    /// - A Header is received after initial data has been written
    /// - An EndOfSequence marker is received
    pub fn should_close_pipe(&self, item: &FlvData) -> bool {
        match item {
            FlvData::Header(_) if self.has_written_data => true,
            FlvData::EndOfSequence(_) => true,
            _ => false,
        }
    }

    /// Write FLV header bytes to the writer
    /// FLV header is 9 bytes + 4 byte previous tag size (0)
    fn write_header<W: Write>(writer: &mut W, header: &FlvHeader) -> io::Result<u64> {
        // Write FLV signature ("FLV")
        writer.write_all(&[0x46, 0x4C, 0x56])?; // "FLV"

        // Write version (0x01)
        writer.write_u8(header.version)?;

        // Write flags (bit 2 for audio, bit 0 for video)
        let mut flags = 0_u8;
        if header.has_audio {
            flags |= 0x04;
        }
        if header.has_video {
            flags |= 0x01;
        }
        writer.write_u8(flags)?;

        // Write data offset (always 9 for standard FLV header)
        writer.write_u32::<BigEndian>(9)?;

        // Write initial previous tag size (0 before first tag)
        writer.write_u32::<BigEndian>(0)?;

        Ok(13) // 9 bytes header + 4 bytes previous tag size
    }

    /// Write FLV tag to the writer
    fn write_tag<W: Write>(writer: &mut W, tag: &FlvTag) -> io::Result<u64> {
        let data_size = tag.data.len() as u32;

        // Write tag type (1 byte)
        writer.write_u8(tag.tag_type.into())?;

        // Write data size (3 bytes)
        writer.write_u24::<BigEndian>(data_size)?;

        // Write timestamp (3 bytes + 1 byte extended)
        writer.write_u24::<BigEndian>(tag.timestamp_ms & 0xFFFFFF)?;
        writer.write_u8((tag.timestamp_ms >> 24) as u8)?;

        // Write stream ID (always 0, 3 bytes)
        writer.write_u24::<BigEndian>(0)?;

        // Write tag data
        writer.write_all(&tag.data)?;

        // Write previous tag size (data size + 11 byte header)
        let previous_tag_size = data_size + 11;
        writer.write_u32::<BigEndian>(previous_tag_size)?;

        // Total bytes: 1 (type) + 3 (size) + 3 (timestamp) + 1 (timestamp ext) + 3 (stream id) + data + 4 (prev tag size)
        // = 11 + data.len() + 4
        Ok((11 + tag.data.len() + 4) as u64)
    }

    /// Get total bytes written
    #[allow(dead_code)]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Check if any data has been written
    #[allow(dead_code)]
    pub fn has_written_data(&self) -> bool {
        self.has_written_data
    }
}

impl FormatStrategy<FlvData> for PipeFlvStrategy {
    type Writer = BufWriter<std::io::Stdout>;
    type StrategyError = PipeFlvStrategyError;

    fn create_writer(&self, _path: &Path) -> Result<Self::Writer, Self::StrategyError> {
        // For pipe output, we always write to stdout
        Ok(BufWriter::with_capacity(64 * 1024, io::stdout()))
    }

    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &FlvData,
    ) -> Result<u64, Self::StrategyError> {
        // Check if this item should trigger pipe closure BEFORE writing
        // This is important for boundary detection
        self.should_close = self.should_close_pipe(item);

        // If we should close, don't write the boundary item (second header or EOS)
        // Just return 0 bytes written and let after_item_written handle the closure
        if self.should_close {
            return Ok(0);
        }

        let bytes_written = match item {
            FlvData::Header(header) => {
                tracing::info!(
                    has_audio = header.has_audio,
                    has_video = header.has_video,
                    version = header.version,
                    "Writing FLV header to pipe"
                );
                let bytes = Self::write_header(writer, header)
                    .map_err(PipeFlvStrategyError::from_io_error)?;
                // Flush after header to ensure it's sent immediately
                writer
                    .flush()
                    .map_err(PipeFlvStrategyError::from_io_error)?;
                tracing::info!(bytes_written = bytes, "FLV header written and flushed");
                self.has_written_data = true;
                bytes
            }
            FlvData::Tag(tag) => {
                let bytes =
                    Self::write_tag(writer, tag).map_err(PipeFlvStrategyError::from_io_error)?;
                self.has_written_data = true;
                bytes
            }
            FlvData::EndOfSequence(_) => {
                tracing::debug!("Received EndOfSequence in pipe strategy");
                // EndOfSequence doesn't write any data, just signals end
                0
            }
        };

        self.bytes_written += bytes_written;
        Ok(bytes_written)
    }

    fn should_rotate_file(&self, _config: &WriterConfig, _state: &WriterState) -> bool {
        // Pipe output doesn't rotate files
        false
    }

    fn next_file_path(&self, config: &WriterConfig, _state: &WriterState) -> PathBuf {
        // For pipe output, we use a dummy path since we're writing to stdout
        config.base_path.join("stdout")
    }

    fn on_file_open(
        &mut self,
        _writer: &mut Self::Writer,
        _path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        // Reset state for new pipe session
        self.has_written_data = false;
        self.should_close = false;
        Ok(0)
    }

    fn on_file_close(
        &mut self,
        writer: &mut Self::Writer,
        _path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        // Flush before closing - handle broken pipe gracefully
        if let Err(e) = writer.flush() {
            if e.kind() == io::ErrorKind::BrokenPipe {
                warn!("Broken pipe during flush: consumer closed the connection");
                // Don't propagate broken pipe on close - it's expected behavior
                return Ok(0);
            }
            return Err(PipeFlvStrategyError::from_io_error(e));
        }
        Ok(0)
    }

    fn after_item_written(
        &mut self,
        item: &FlvData,
        _bytes_written: u64,
        _state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        // Signal closure if a boundary was detected BEFORE writing
        // We use self.should_close which was set in write_item before has_written_data was updated
        if self.should_close {
            self.segment_count += 1;

            // Log segment boundary event to stderr
            let boundary_type = match item {
                FlvData::Header(_) => "FLV Header",
                FlvData::EndOfSequence(_) => "FLV EndOfSequence",
                _ => "Unknown",
            };
            tracing::info!(
                boundary_type = boundary_type,
                segment_count = self.segment_count,
                bytes_written = self.bytes_written,
                "Segment boundary detected"
            );

            Ok(PostWriteAction::Close)
        } else {
            Ok(PostWriteAction::None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::tag::{FlvTag, FlvTagType};
    use std::sync::{Arc, Mutex};

    /// A thread-safe wrapper around a Vec<u8> that implements Write
    #[derive(Clone)]
    struct SharedBuffer {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedBuffer {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_data(&self) -> Vec<u8> {
            self.inner.lock().unwrap().clone()
        }

        #[allow(dead_code)]
        fn clear(&self) {
            self.inner.lock().unwrap().clear();
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut inner = self.inner.lock().unwrap();
            inner.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    unsafe impl Send for SharedBuffer {}
    unsafe impl Sync for SharedBuffer {}

    /// Create a test FLV header
    fn create_test_header(has_audio: bool, has_video: bool) -> FlvHeader {
        FlvHeader {
            signature: 0x464C56, // "FLV"
            version: 0x01,
            has_audio,
            has_video,
            data_offset: Bytes::new(),
        }
    }

    /// Create a test FLV tag
    fn create_test_tag(tag_type: FlvTagType, timestamp_ms: u32, data: Vec<u8>) -> FlvTag {
        FlvTag {
            timestamp_ms,
            stream_id: 0,
            tag_type,
            data: Bytes::from(data),
        }
    }

    #[test]
    fn test_should_close_pipe_first_header() {
        let strategy = PipeFlvStrategy::new();
        let header = FlvData::Header(create_test_header(true, true));

        // First header should NOT trigger closure
        assert!(!strategy.should_close_pipe(&header));
    }

    #[test]
    fn test_should_close_pipe_second_header() {
        let mut strategy = PipeFlvStrategy::new();
        strategy.has_written_data = true;

        let header = FlvData::Header(create_test_header(true, true));

        // Second header (after data written) SHOULD trigger closure
        assert!(strategy.should_close_pipe(&header));
    }

    #[test]
    fn test_should_close_pipe_end_of_sequence() {
        let strategy = PipeFlvStrategy::new();
        let eos = FlvData::EndOfSequence(Bytes::new());

        // EndOfSequence should always trigger closure
        assert!(strategy.should_close_pipe(&eos));
    }

    #[test]
    fn test_should_close_pipe_tag() {
        let mut strategy = PipeFlvStrategy::new();
        strategy.has_written_data = true;

        let tag = FlvData::Tag(create_test_tag(FlvTagType::Video, 0, vec![0x17, 0x00]));

        // Regular tag should NOT trigger closure
        assert!(!strategy.should_close_pipe(&tag));
    }

    #[test]
    fn test_write_header_bytes() {
        let buffer = SharedBuffer::new();
        let mut writer = BufWriter::new(Box::new(buffer.clone()) as Box<dyn Write + Send + Sync>);

        let header = create_test_header(true, true);
        let bytes_written = PipeFlvStrategy::write_header(&mut writer, &header).unwrap();
        writer.flush().unwrap();

        assert_eq!(bytes_written, 13); // 9 bytes header + 4 bytes prev tag size

        let data = buffer.get_data();
        assert_eq!(&data[0..3], b"FLV"); // Signature
        assert_eq!(data[3], 0x01); // Version
        assert_eq!(data[4], 0x05); // Flags (audio + video)
        assert_eq!(&data[5..9], &[0x00, 0x00, 0x00, 0x09]); // Data offset
        assert_eq!(&data[9..13], &[0x00, 0x00, 0x00, 0x00]); // Previous tag size
    }

    #[test]
    fn test_write_tag_bytes() {
        let buffer = SharedBuffer::new();
        let mut writer = BufWriter::new(Box::new(buffer.clone()) as Box<dyn Write + Send + Sync>);

        let tag_data = vec![0x17, 0x00, 0x00, 0x00, 0x00]; // 5 bytes of data
        let tag = create_test_tag(FlvTagType::Video, 1000, tag_data.clone());
        let bytes_written = PipeFlvStrategy::write_tag(&mut writer, &tag).unwrap();
        writer.flush().unwrap();

        // 11 bytes header + 5 bytes data + 4 bytes prev tag size = 20
        assert_eq!(bytes_written, 20);

        let data = buffer.get_data();
        assert_eq!(data[0], 9); // Video tag type
        assert_eq!(&data[1..4], &[0x00, 0x00, 0x05]); // Data size (5)
        // Timestamp: 1000 = 0x3E8
        assert_eq!(&data[4..7], &[0x00, 0x03, 0xE8]); // Timestamp lower 24 bits
        assert_eq!(data[7], 0x00); // Timestamp extended
        assert_eq!(&data[8..11], &[0x00, 0x00, 0x00]); // Stream ID
        assert_eq!(&data[11..16], &tag_data[..]); // Tag data
        // Previous tag size: 11 + 5 = 16 = 0x10
        assert_eq!(&data[16..20], &[0x00, 0x00, 0x00, 0x10]);
    }
}
