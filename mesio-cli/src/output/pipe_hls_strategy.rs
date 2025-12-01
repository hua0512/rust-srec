//! HLS Pipe Output Strategy
//!
//! This module provides a strategy for writing HLS data to pipe output (stdout).
//! It implements segment boundary detection to close the pipe when a discontinuity
//! or EndMarker is received.

use hls::{HlsData, M4sData};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

use pipeline_common::{FormatStrategy, PostWriteAction, WriterConfig, WriterState};

/// Error type for pipe HLS strategy
#[derive(Debug, Error)]
pub enum PipeHlsStrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Broken pipe: consumer closed the connection")]
    BrokenPipe,
}

impl PipeHlsStrategyError {
    /// Check if this error is a broken pipe error
    #[allow(dead_code)]
    pub fn is_broken_pipe(&self) -> bool {
        match self {
            PipeHlsStrategyError::BrokenPipe => true,
            PipeHlsStrategyError::Io(e) => e.kind() == io::ErrorKind::BrokenPipe,
        }
    }

    /// Create a broken pipe error from an I/O error if applicable
    pub fn from_io_error(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::BrokenPipe {
            warn!("Broken pipe detected: consumer closed the connection");
            PipeHlsStrategyError::BrokenPipe
        } else {
            PipeHlsStrategyError::Io(err)
        }
    }
}

/// HLS pipe output strategy
///
/// This strategy writes HLS data to a pipe (stdout) and detects segment boundaries
/// to signal when the pipe should be closed. Boundaries are detected when:
/// - A discontinuity flag is set on a segment
/// - An EndMarker is received
pub struct PipeHlsStrategy {
    /// Whether any data has been written to the current pipe
    has_written_data: bool,
    /// Total bytes written
    bytes_written: u64,
    /// Whether a boundary was detected that should trigger pipe closure
    should_close: bool,
    /// Count of discontinuities encountered
    discontinuity_count: u32,
}

impl Default for PipeHlsStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeHlsStrategy {
    /// Create a new PipeHlsStrategy
    pub fn new() -> Self {
        Self {
            has_written_data: false,
            bytes_written: 0,
            should_close: false,
            discontinuity_count: 0,
        }
    }

    /// Check if the pipe should be closed based on the current item
    ///
    /// Returns true when:
    /// - A segment with discontinuity flag is received
    /// - An EndMarker is received
    pub fn should_close_pipe(&self, item: &HlsData) -> bool {
        match item {
            HlsData::EndMarker => true,
            _ if item.is_discontinuity() => true,
            _ => false,
        }
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

    /// Get the count of discontinuities encountered
    #[allow(dead_code)]
    pub fn discontinuity_count(&self) -> u32 {
        self.discontinuity_count
    }

    /// Write an HLS data item to any writer.
    /// This is the core write logic, usable with any `Write` implementation.
    pub fn write_to<W: io::Write>(
        &mut self,
        writer: &mut W,
        item: &HlsData,
    ) -> Result<u64, PipeHlsStrategyError> {
        // Check if this item should trigger pipe closure BEFORE writing
        // This is important for boundary detection
        self.should_close = self.should_close_pipe(item);

        // Track discontinuities
        if item.is_discontinuity() {
            self.discontinuity_count += 1;
        }

        let bytes_written = match item {
            HlsData::TsData(ts) => {
                writer
                    .write_all(&ts.data)
                    .map_err(PipeHlsStrategyError::from_io_error)?;
                self.has_written_data = true;
                ts.data.len() as u64
            }
            HlsData::M4sData(m4s_data) => {
                let bytes = match m4s_data {
                    M4sData::InitSegment(init) => {
                        writer
                            .write_all(&init.data)
                            .map_err(PipeHlsStrategyError::from_io_error)?;
                        init.data.len() as u64
                    }
                    M4sData::Segment(segment) => {
                        writer
                            .write_all(&segment.data)
                            .map_err(PipeHlsStrategyError::from_io_error)?;
                        segment.data.len() as u64
                    }
                };
                self.has_written_data = true;
                bytes
            }
            HlsData::EndMarker => {
                // EndMarker doesn't write any data, just signals end
                0
            }
        };

        self.bytes_written += bytes_written;
        Ok(bytes_written)
    }
}

impl FormatStrategy<HlsData> for PipeHlsStrategy {
    type Writer = BufWriter<std::io::Stdout>;
    type StrategyError = PipeHlsStrategyError;

    fn create_writer(&self, _path: &Path) -> Result<Self::Writer, Self::StrategyError> {
        // For pipe output, we always write to stdout
        Ok(BufWriter::with_capacity(64 * 1024, io::stdout()))
    }

    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &HlsData,
    ) -> Result<u64, Self::StrategyError> {
        self.write_to(writer, item)
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
            return Err(PipeHlsStrategyError::from_io_error(e));
        }
        Ok(0)
    }

    fn after_item_written(
        &mut self,
        item: &HlsData,
        _bytes_written: u64,
        _state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        // Signal closure if a boundary was detected
        if self.should_close_pipe(item) {
            // Log segment boundary event to stderr
            let boundary_type = match item {
                HlsData::EndMarker => "HLS EndMarker",
                _ if item.is_discontinuity() => "HLS Discontinuity",
                _ => "Unknown",
            };
            tracing::info!(
                boundary_type = boundary_type,
                segment_count = self.discontinuity_count,
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

    /// Create a test TS segment using HlsData::ts constructor
    fn create_test_ts_segment(discontinuity: bool, data: Vec<u8>) -> HlsData {
        // Use the HlsData::ts constructor which creates a proper MediaSegment internally
        HlsData::ts(
            m3u8_rs::MediaSegment {
                uri: "test.ts".to_string(),
                duration: 10.0,
                title: None,
                byte_range: None,
                discontinuity,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data),
        )
    }

    /// Create a test M4S init segment
    fn create_test_m4s_init_segment(discontinuity: bool, data: Vec<u8>) -> HlsData {
        HlsData::mp4_init(
            m3u8_rs::MediaSegment {
                uri: "init.mp4".to_string(),
                duration: 0.0,
                title: None,
                byte_range: None,
                discontinuity,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data),
        )
    }

    /// Create a test M4S media segment
    fn create_test_m4s_segment(discontinuity: bool, data: Vec<u8>) -> HlsData {
        HlsData::mp4_segment(
            m3u8_rs::MediaSegment {
                uri: "segment.m4s".to_string(),
                duration: 10.0,
                title: None,
                byte_range: None,
                discontinuity,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data),
        )
    }

    #[test]
    fn test_should_close_pipe_end_marker() {
        let strategy = PipeHlsStrategy::new();
        let end_marker = HlsData::EndMarker;

        // EndMarker should always trigger closure
        assert!(strategy.should_close_pipe(&end_marker));
    }

    #[test]
    fn test_should_close_pipe_discontinuity_ts() {
        let strategy = PipeHlsStrategy::new();
        let ts_with_discontinuity = create_test_ts_segment(true, vec![0x47, 0x00, 0x00]);

        // TS segment with discontinuity should trigger closure
        assert!(strategy.should_close_pipe(&ts_with_discontinuity));
    }

    #[test]
    fn test_should_close_pipe_no_discontinuity_ts() {
        let strategy = PipeHlsStrategy::new();
        let ts_without_discontinuity = create_test_ts_segment(false, vec![0x47, 0x00, 0x00]);

        // TS segment without discontinuity should NOT trigger closure
        assert!(!strategy.should_close_pipe(&ts_without_discontinuity));
    }

    #[test]
    fn test_should_close_pipe_discontinuity_m4s() {
        let strategy = PipeHlsStrategy::new();
        let m4s_with_discontinuity = create_test_m4s_segment(true, vec![0x00, 0x00, 0x00]);

        // M4S segment with discontinuity should trigger closure
        assert!(strategy.should_close_pipe(&m4s_with_discontinuity));
    }

    #[test]
    fn test_write_ts_segment() {
        let mut buffer = SharedBuffer::new();

        let mut strategy = PipeHlsStrategy::new();
        let test_data = vec![0x47, 0x00, 0x11, 0x10, 0x00];
        let ts_segment = create_test_ts_segment(false, test_data.clone());

        let bytes_written = strategy.write_to(&mut buffer, &ts_segment).unwrap();

        assert_eq!(bytes_written, test_data.len() as u64);
        assert_eq!(buffer.get_data(), test_data);
        assert!(strategy.has_written_data());
    }

    #[test]
    fn test_write_m4s_init_segment() {
        let mut buffer = SharedBuffer::new();

        let mut strategy = PipeHlsStrategy::new();
        let test_data = vec![0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p'];
        let m4s_init = create_test_m4s_init_segment(false, test_data.clone());

        let bytes_written = strategy.write_to(&mut buffer, &m4s_init).unwrap();

        assert_eq!(bytes_written, test_data.len() as u64);
        assert_eq!(buffer.get_data(), test_data);
        assert!(strategy.has_written_data());
    }

    #[test]
    fn test_write_m4s_media_segment() {
        let mut buffer = SharedBuffer::new();

        let mut strategy = PipeHlsStrategy::new();
        let test_data = vec![0x00, 0x00, 0x00, 0x08, b'm', b'o', b'o', b'f'];
        let m4s_segment = create_test_m4s_segment(false, test_data.clone());

        let bytes_written = strategy.write_to(&mut buffer, &m4s_segment).unwrap();

        assert_eq!(bytes_written, test_data.len() as u64);
        assert_eq!(buffer.get_data(), test_data);
        assert!(strategy.has_written_data());
    }

    #[test]
    fn test_write_end_marker() {
        let mut buffer = SharedBuffer::new();

        let mut strategy = PipeHlsStrategy::new();
        let end_marker = HlsData::EndMarker;

        let bytes_written = strategy.write_to(&mut buffer, &end_marker).unwrap();

        // EndMarker should write 0 bytes
        assert_eq!(bytes_written, 0);
        assert!(buffer.get_data().is_empty());
    }

    #[test]
    fn test_discontinuity_count() {
        let mut buffer = SharedBuffer::new();

        let mut strategy = PipeHlsStrategy::new();

        // Write a segment with discontinuity
        let ts_with_disc = create_test_ts_segment(true, vec![0x47]);
        strategy.write_to(&mut buffer, &ts_with_disc).unwrap();
        assert_eq!(strategy.discontinuity_count(), 1);

        // Write a segment without discontinuity
        let ts_without_disc = create_test_ts_segment(false, vec![0x47]);
        strategy.write_to(&mut buffer, &ts_without_disc).unwrap();
        assert_eq!(strategy.discontinuity_count(), 1);

        // Write another segment with discontinuity
        let ts_with_disc2 = create_test_ts_segment(true, vec![0x47]);
        strategy.write_to(&mut buffer, &ts_with_disc2).unwrap();
        assert_eq!(strategy.discontinuity_count(), 2);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use bytes::Bytes;
    use proptest::prelude::*;
    use std::io::Write;
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

        #[allow(dead_code)]
        fn get_data(&self) -> Vec<u8> {
            self.inner.lock().unwrap().clone()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut inner = self.inner.lock().unwrap();
            inner.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    unsafe impl Send for SharedBuffer {}
    unsafe impl Sync for SharedBuffer {}

    /// Create a test TS segment with given discontinuity flag
    fn create_ts_segment(discontinuity: bool, data: Vec<u8>) -> HlsData {
        HlsData::ts(
            m3u8_rs::MediaSegment {
                uri: "test.ts".to_string(),
                duration: 10.0,
                title: None,
                byte_range: None,
                discontinuity,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data),
        )
    }

    /// Create a test M4S segment with given discontinuity flag
    fn create_m4s_segment(discontinuity: bool, data: Vec<u8>) -> HlsData {
        HlsData::mp4_segment(
            m3u8_rs::MediaSegment {
                uri: "segment.m4s".to_string(),
                duration: 10.0,
                title: None,
                byte_range: None,
                discontinuity,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Feature: data-pipelining-output, Property 4: HLS discontinuity triggers pipe closure**
        /// *For any* HLS stream being written to pipe output, when a segment with the discontinuity
        /// flag set is encountered, the pipe closure signal should be triggered.
        /// **Validates: Requirements 3.1**
        #[test]
        fn prop_hls_discontinuity_triggers_pipe_closure(
            data in proptest::collection::vec(any::<u8>(), 1..100),
            is_ts in any::<bool>(),
        ) {
            let strategy = PipeHlsStrategy::new();

            // Create a segment with discontinuity flag set
            let segment = if is_ts {
                create_ts_segment(true, data)
            } else {
                create_m4s_segment(true, data)
            };

            // Discontinuity should always trigger closure
            prop_assert!(
                strategy.should_close_pipe(&segment),
                "HLS segment with discontinuity flag should trigger pipe closure"
            );
        }

        /// **Feature: data-pipelining-output, Property 4 (negative): Non-discontinuity segments don't trigger closure**
        /// *For any* HLS segment without the discontinuity flag, the pipe should NOT be closed.
        /// **Validates: Requirements 3.1**
        #[test]
        fn prop_hls_non_discontinuity_does_not_trigger_closure(
            data in proptest::collection::vec(any::<u8>(), 1..100),
            is_ts in any::<bool>(),
        ) {
            let strategy = PipeHlsStrategy::new();

            // Create a segment without discontinuity flag
            let segment = if is_ts {
                create_ts_segment(false, data)
            } else {
                create_m4s_segment(false, data)
            };

            // Non-discontinuity should NOT trigger closure
            prop_assert!(
                !strategy.should_close_pipe(&segment),
                "HLS segment without discontinuity flag should NOT trigger pipe closure"
            );
        }

        /// **Feature: data-pipelining-output, Property 5: HLS EndMarker triggers pipe closure**
        /// *For any* HLS stream being written to pipe output, when an EndMarker is received,
        /// the pipe closure signal should be triggered.
        /// **Validates: Requirements 3.3**
        #[test]
        fn prop_hls_end_marker_triggers_pipe_closure(
            has_written_data in any::<bool>(),
        ) {
            let mut strategy = PipeHlsStrategy::new();
            strategy.has_written_data = has_written_data;

            let end_marker = HlsData::EndMarker;

            // EndMarker should always trigger closure regardless of state
            prop_assert!(
                strategy.should_close_pipe(&end_marker),
                "HLS EndMarker should always trigger pipe closure"
            );
        }

        /// **Feature: data-pipelining-output, Property 7: Multiple discontinuities handled correctly**
        /// *For any* HLS stream with N discontinuity events, exactly N pipe closure signals
        /// should be generated, one for each discontinuity.
        /// **Validates: Requirements 3.4**
        #[test]
        fn prop_multiple_discontinuities_handled_correctly(
            discontinuity_flags in proptest::collection::vec(any::<bool>(), 1..20),
            data_sizes in proptest::collection::vec(1..100usize, 1..20),
        ) {
            let mut buffer = SharedBuffer::new();

            let mut strategy = PipeHlsStrategy::new();

            // Count expected discontinuities
            let expected_discontinuity_count: u32 = discontinuity_flags
                .iter()
                .filter(|&&d| d)
                .count() as u32;

            // Count closure signals
            let mut closure_signals = 0u32;

            // Process each segment
            for (i, &discontinuity) in discontinuity_flags.iter().enumerate() {
                let data_size = data_sizes.get(i).copied().unwrap_or(10);
                let data = vec![0x47u8; data_size]; // TS sync byte pattern
                let segment = create_ts_segment(discontinuity, data);

                // Write the item
                strategy.write_to(&mut buffer, &segment).unwrap();

                // Check if closure was signaled
                if strategy.should_close_pipe(&segment) {
                    closure_signals += 1;
                }
            }

            // Verify discontinuity count matches
            prop_assert_eq!(
                strategy.discontinuity_count(),
                expected_discontinuity_count,
                "Discontinuity count should match number of segments with discontinuity flag"
            );

            // Verify closure signals match discontinuity count
            prop_assert_eq!(
                closure_signals,
                expected_discontinuity_count,
                "Number of closure signals should equal number of discontinuities"
            );
        }
    }
}
