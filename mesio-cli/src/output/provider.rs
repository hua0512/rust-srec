#![allow(dead_code)]
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use clap::ValueEnum;
use tracing::{debug, info};

/// Segment boundary events that trigger pipe closure
/// Used to signal logical breaks in the stream where downstream consumers
/// may need to handle segments independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentBoundaryEvent {
    /// A new FLV header was received, indicating a new segment
    FlvHeader,
    /// An FLV EndOfSequence marker was received, indicating stream end
    FlvEndOfSequence,
    /// An HLS discontinuity tag was encountered
    HlsDiscontinuity,
    /// An HLS EndMarker was received, indicating stream end
    HlsEndMarker,
}

/// OutputFormat enum to specify the type of output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Write to a file
    #[default]
    File,
    /// Write to stdout
    Stdout,
    /// Write to stderr
    Stderr,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(OutputFormat::File),
            "stdout" => Ok(OutputFormat::Stdout),
            "stderr" => Ok(OutputFormat::Stderr),
            _ => Err(format!(
                "Unknown output format: '{}'. Valid values are: file, stdout, stderr",
                s
            )),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::File => write!(f, "file"),
            OutputFormat::Stdout => write!(f, "stdout"),
            OutputFormat::Stderr => write!(f, "stderr"),
        }
    }
}

/// A trait defining the interface for output providers
pub trait OutputProvider: Send + Sync {
    /// Write bytes to the output
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize>;

    /// Flush any buffered data
    fn flush(&mut self) -> io::Result<()>;

    /// Get total bytes written so far
    fn bytes_written(&self) -> u64;

    /// Close the provider and perform any necessary cleanup
    fn close(&mut self) -> io::Result<()>;
}

/// A file-based output provider
pub struct FileOutputProvider {
    writer: BufWriter<File>,
    bytes_written: u64,
}

impl FileOutputProvider {
    /// Create a new file output provider
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let file = File::create(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            bytes_written: 0,
        })
    }
}

impl OutputProvider for FileOutputProvider {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let bytes_written = self.writer.write(bytes)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn close(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// A pipe-based output provider with segment boundary detection support
pub struct PipeOutputProvider {
    writer: BufWriter<Box<dyn Write + Send + Sync>>,
    bytes_written: u64,
    /// Count of segment boundaries encountered
    segment_count: u32,
    /// Whether to signal closure on segment boundaries
    close_on_boundary: bool,
}

impl PipeOutputProvider {
    /// Create a new stdout output provider
    pub fn stdout() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(Box::new(io::stdout())),
            bytes_written: 0,
            segment_count: 0,
            close_on_boundary: false,
        })
    }

    /// Create a new stderr output provider
    pub fn stderr() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(Box::new(io::stderr())),
            bytes_written: 0,
            segment_count: 0,
            close_on_boundary: false,
        })
    }

    /// Create a new stdout output provider with boundary detection enabled
    pub fn stdout_with_boundary_detection() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(Box::new(io::stdout())),
            bytes_written: 0,
            segment_count: 0,
            close_on_boundary: true,
        })
    }

    /// Get the current segment count
    pub fn segment_count(&self) -> u32 {
        self.segment_count
    }

    /// Check if boundary detection is enabled
    pub fn close_on_boundary(&self) -> bool {
        self.close_on_boundary
    }

    /// Set whether to close on segment boundaries
    pub fn set_close_on_boundary(&mut self, close: bool) {
        self.close_on_boundary = close;
    }

    /// Handle a segment boundary event
    /// Flushes buffered data and signals whether the pipe should be closed
    /// Returns Ok(true) if the pipe should be closed, Ok(false) otherwise
    pub fn on_segment_boundary(&mut self, event: SegmentBoundaryEvent) -> io::Result<bool> {
        self.segment_count += 1;
        debug!(
            "Segment boundary detected: {:?}, count: {}",
            event, self.segment_count
        );

        // Always flush on boundary
        self.flush()?;

        // Signal closure if configured to do so
        Ok(self.close_on_boundary)
    }

    /// Create a new pipe output provider with a custom writer (for testing)
    #[cfg(test)]
    pub fn with_writer(writer: Box<dyn Write + Send + Sync>, close_on_boundary: bool) -> Self {
        Self {
            writer: BufWriter::new(writer),
            bytes_written: 0,
            segment_count: 0,
            close_on_boundary,
        }
    }
}

impl OutputProvider for PipeOutputProvider {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let bytes_written = self.writer.write(bytes)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn close(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// Output manager that handles creating and managing output providers
pub struct OutputManager {
    provider: Box<dyn OutputProvider>,
}

impl OutputManager {
    /// Create a new output manager with a specific output format
    pub fn new(format: OutputFormat, output_path: Option<PathBuf>) -> io::Result<Self> {
        let provider: Box<dyn OutputProvider> = match format {
            OutputFormat::File => {
                let path = output_path.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Output path required for file output",
                    )
                })?;
                Box::new(FileOutputProvider::new(path)?)
            }
            OutputFormat::Stdout => Box::new(PipeOutputProvider::stdout()?),
            OutputFormat::Stderr => Box::new(PipeOutputProvider::stderr()?),
        };

        Ok(Self { provider })
    }

    /// Create a new output manager with a custom provider (for testing)
    #[cfg(test)]
    pub fn with_provider(provider: Box<dyn OutputProvider>) -> Self {
        Self { provider }
    }

    /// Get the total bytes written
    pub fn bytes_written(&self) -> u64 {
        self.provider.bytes_written()
    }

    /// Write data to the output provider with progress updates
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.provider.write(bytes)
    }

    /// Write bytes from the Bytes type
    pub fn write_bytes(&mut self, bytes: &Bytes) -> io::Result<usize> {
        self.write(bytes)
    }

    /// Flush the output
    pub fn flush(&mut self) -> io::Result<()> {
        self.provider.flush()
    }

    /// Close the output and finalize
    pub fn close(mut self) -> io::Result<u64> {
        self.flush()?;
        self.provider.close()?;

        // Progress updates are now handled by the event system

        Ok(self.provider.bytes_written())
    }
}

/// Create an output provider based on the format and configuration
pub fn create_output(
    format: OutputFormat,
    output_dir: &Path,
    base_name: &str,
    extension: &str,
) -> io::Result<OutputManager> {
    match format {
        OutputFormat::File => {
            // Ensure output directory exists
            std::fs::create_dir_all(output_dir)?;

            let path = output_dir.join(format!("{base_name}.{extension}"));
            info!("Creating file output: {}", path.display());

            OutputManager::new(format, Some(path))
        }
        OutputFormat::Stdout => {
            debug!("Creating stdout output");
            OutputManager::new(format, None)
        }
        OutputFormat::Stderr => {
            debug!("Creating stderr output");
            OutputManager::new(format, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::{Arc, Mutex};

    /// State tracking for flush operations
    #[derive(Clone, Default)]
    struct FlushTracker {
        /// Number of times flush was called
        flush_count: Arc<Mutex<u32>>,
        /// Data written before each flush (snapshot at flush time)
        data_at_flush: Arc<Mutex<Vec<Vec<u8>>>>,
        /// Current data buffer
        data: Arc<Mutex<Vec<u8>>>,
    }

    impl FlushTracker {
        fn new() -> Self {
            Self {
                flush_count: Arc::new(Mutex::new(0)),
                data_at_flush: Arc::new(Mutex::new(Vec::new())),
                data: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn flush_count(&self) -> u32 {
            *self.flush_count.lock().unwrap()
        }

        fn data_at_flush(&self, index: usize) -> Option<Vec<u8>> {
            self.data_at_flush.lock().unwrap().get(index).cloned()
        }

        fn get_data(&self) -> Vec<u8> {
            self.data.lock().unwrap().clone()
        }
    }

    impl Write for FlushTracker {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut data = self.data.lock().unwrap();
            data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut flush_count = self.flush_count.lock().unwrap();
            *flush_count += 1;

            // Snapshot the current data at flush time
            let data = self.data.lock().unwrap().clone();
            self.data_at_flush.lock().unwrap().push(data);

            Ok(())
        }
    }

    unsafe impl Send for FlushTracker {}
    unsafe impl Sync for FlushTracker {}

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

    // Make SharedBuffer Send + Sync
    unsafe impl Send for SharedBuffer {}
    unsafe impl Sync for SharedBuffer {}

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Feature: data-pipelining-output, Property 1: Binary data integrity through pipe output**
        /// *For any* sequence of bytes written to the pipe output provider, reading those bytes
        /// from the output should produce an identical byte sequence.
        /// **Validates: Requirements 1.1, 1.3**
        #[test]
        fn prop_binary_data_integrity(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let buffer = SharedBuffer::new();
            let buffer_clone = buffer.clone();

            let mut provider = PipeOutputProvider::with_writer(
                Box::new(buffer),
                false,
            );

            // Write the data
            let bytes_written = provider.write(&data).unwrap();
            provider.flush().unwrap();

            // Verify bytes written count
            prop_assert_eq!(bytes_written, data.len());
            prop_assert_eq!(provider.bytes_written(), data.len() as u64);

            // Verify the data integrity
            let output = buffer_clone.get_data();
            prop_assert_eq!(output, data, "Output data should match input data exactly");
        }

        /// **Feature: data-pipelining-output, Property 8: OutputManager lifecycle management**
        /// *For any* OutputManager instance, the sequence of operations (create → write* → flush → close)
        /// should maintain consistent state and bytes_written count should equal the sum of all
        /// individual write operations.
        /// **Validates: Requirements 5.3**
        #[test]
        fn prop_output_manager_lifecycle(
            write_chunks in proptest::collection::vec(
                proptest::collection::vec(any::<u8>(), 1..256),
                1..10
            )
        ) {
            let buffer = SharedBuffer::new();
            let buffer_clone = buffer.clone();

            let provider = PipeOutputProvider::with_writer(
                Box::new(buffer),
                false,
            );

            let mut manager = OutputManager::with_provider(Box::new(provider));

            // Track expected total bytes
            let mut expected_total: u64 = 0;

            // Write multiple chunks
            for chunk in &write_chunks {
                let written = manager.write(chunk).unwrap();
                prop_assert_eq!(written, chunk.len(), "Each write should return the number of bytes written");
                expected_total += written as u64;

                // Verify running total
                prop_assert_eq!(
                    manager.bytes_written(),
                    expected_total,
                    "bytes_written should equal sum of all writes so far"
                );
            }

            // Flush should succeed
            manager.flush().unwrap();

            // Close should return the total bytes written
            let final_bytes = manager.close().unwrap();
            prop_assert_eq!(
                final_bytes,
                expected_total,
                "close() should return total bytes written"
            );

            // Verify the actual data written
            let output = buffer_clone.get_data();
            let expected_data: Vec<u8> = write_chunks.into_iter().flatten().collect();
            prop_assert_eq!(output, expected_data, "Output data should match all written chunks concatenated");
        }

        /// **Feature: data-pipelining-output, Property 6: Flush before close on segment boundary**
        /// *For any* segment boundary event (FLV header, FLV EndOfSequence, HLS discontinuity, HLS EndMarker),
        /// all buffered data should be flushed before the pipe is closed.
        /// **Validates: Requirements 2.2, 3.2**
        #[test]
        fn prop_flush_before_close_on_segment_boundary(
            data_chunks in proptest::collection::vec(
                proptest::collection::vec(any::<u8>(), 1..100),
                1..5
            ),
            event_type in prop_oneof![
                Just(SegmentBoundaryEvent::FlvHeader),
                Just(SegmentBoundaryEvent::FlvEndOfSequence),
                Just(SegmentBoundaryEvent::HlsDiscontinuity),
                Just(SegmentBoundaryEvent::HlsEndMarker),
            ],
        ) {
            let tracker = FlushTracker::new();
            let tracker_clone = tracker.clone();

            let mut provider = PipeOutputProvider::with_writer(
                Box::new(tracker),
                true, // Enable close on boundary
            );

            // Write data chunks
            let mut total_written = 0usize;
            for chunk in &data_chunks {
                provider.write(chunk).unwrap();
                total_written += chunk.len();
            }

            // Verify no flush has occurred yet
            prop_assert_eq!(
                tracker_clone.flush_count(),
                0,
                "No flush should occur before segment boundary"
            );

            // Trigger segment boundary - this should flush before signaling closure
            let should_close = provider.on_segment_boundary(event_type).unwrap();

            // Verify flush was called
            prop_assert_eq!(
                tracker_clone.flush_count(),
                1,
                "Flush should be called exactly once on segment boundary"
            );

            // Verify all data was present at flush time
            let data_at_flush = tracker_clone.data_at_flush(0).unwrap();
            let expected_data: Vec<u8> = data_chunks.into_iter().flatten().collect();
            prop_assert_eq!(
                data_at_flush.len(),
                total_written,
                "All written data should be present at flush time"
            );
            prop_assert_eq!(
                data_at_flush,
                expected_data,
                "Data at flush should match all written chunks"
            );

            // Verify closure was signaled (since close_on_boundary is true)
            prop_assert!(
                should_close,
                "Closure should be signaled when close_on_boundary is enabled"
            );

            // Verify segment count was incremented
            prop_assert_eq!(
                provider.segment_count(),
                1,
                "Segment count should be incremented on boundary"
            );
        }

        /// **Feature: data-pipelining-output, Property 6 (multiple boundaries): Flush before each close**
        /// *For any* sequence of segment boundary events, flush should be called before each closure signal.
        /// **Validates: Requirements 2.2, 3.2**
        #[test]
        fn prop_flush_before_each_boundary_close(
            boundary_count in 1..5usize,
            data_size in 1..50usize,
        ) {
            let tracker = FlushTracker::new();
            let tracker_clone = tracker.clone();

            let mut provider = PipeOutputProvider::with_writer(
                Box::new(tracker),
                true,
            );

            let events = [
                SegmentBoundaryEvent::FlvHeader,
                SegmentBoundaryEvent::FlvEndOfSequence,
                SegmentBoundaryEvent::HlsDiscontinuity,
                SegmentBoundaryEvent::HlsEndMarker,
            ];

            for i in 0..boundary_count {
                // Write some data before each boundary
                let data = vec![i as u8; data_size];
                provider.write(&data).unwrap();

                // Trigger boundary
                let event = events[i % events.len()];
                provider.on_segment_boundary(event).unwrap();

                // Verify flush count matches boundary count
                prop_assert_eq!(
                    tracker_clone.flush_count(),
                    (i + 1) as u32,
                    "Flush should be called for each segment boundary"
                );
            }

            // Verify total segment count
            prop_assert_eq!(
                provider.segment_count(),
                boundary_count as u32,
                "Segment count should match number of boundaries"
            );
        }
    }
}
