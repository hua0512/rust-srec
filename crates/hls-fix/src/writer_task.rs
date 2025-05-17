//! # HLS Writer Task Module
//!
//! This module provides functionality for writing HLS (HTTP Live Streaming) data
//! to files and collecting statistics about the content.
//!
//! ## Key Features:
//!
//! - Handles sequential HLS segment writing from stream sources
//! - Manages both initialization and media segments
//! - Supports different segment formats (TS, fMP4)
//! - Collects and analyzes segment metadata for statistical reporting
//! - Provides callback mechanisms for reporting progress and events
//!
//! ## Design:
//!
//! The module uses a sequential processing approach where each segment is:
//! 1. Analyzed to extract metadata and statistics
//! 2. Written to disk with appropriate naming
//! 3. Tracked for progress reporting
//!
//! This design allows for straightforward processing without requiring complex
//! thread synchronization, while still providing rich feedback through callbacks.
//!
//! ## Primary Components:
//!
//! - `HlsWriterTask`: Main struct that manages the HLS writing process
//! - `WriterError`: Custom error type for writer-related failures
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use crate::analyzer::HlsAnalyzer;
use hls::{HlsData, SegmentType};
use std::sync::mpsc::Receiver;
use std::{
    error::Error,
    fs,
    io::{self, BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
use tracing::{debug, info, warn};

// Custom Error type
#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),
    #[error("Task Join Error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Writer state error: {0}")]
    State(String),
    #[error("Pipeline error: {0}")]
    Pipeline(#[from] pipeline_common::PipelineError),
}

/// Type alias for the status callback function that provides current download statistics
pub type StatusCallback = dyn Fn(Option<&PathBuf>, u64, f64, Option<u32>) + Send + 'static;

/// Type alias for the segment open callback function
pub type SegmentOpenCallback = dyn Fn(&PathBuf, SegmentType) + Send + 'static;

/// Type alias for the segment close callback function
pub type SegmentCloseCallback = dyn Fn(&PathBuf, SegmentType, u64, f32) + Send + 'static;

/// Manages the writing of processed HLS data to output files using synchronous I/O
/// delegated via spawn_blocking.
pub struct HlsWriterTask {
    output_dir: PathBuf,
    base_name: String,

    // Analyzer for collecting statistics
    analyzer: HlsAnalyzer,

    // State tracking
    current_file_writer: Option<BufWriter<fs::File>>,
    current_file_path: Option<PathBuf>,
    current_file_size: u64,
    current_file_start_instant: Option<Instant>,
    current_segment_type: Option<SegmentType>,
    current_segment_duration: f32,

    // Segment counters for each type
    ts_segment_count: u32,
    mp4_segment_count: u32,
    init_segment_count: u32,

    // Total segment counts
    total_segment_count: u64,

    // Status callback
    status_callback: Option<Box<StatusCallback>>,

    // Segment open/close callbacks
    on_segment_open: Option<Box<SegmentOpenCallback>>,
    on_segment_close: Option<Box<SegmentCloseCallback>>,

    // Use base name directly
    use_base_name_directly: bool,
}

impl HlsWriterTask {
    /// Creates a new writer task and ensures the output directory exists.
    pub fn new(output_dir: PathBuf, base_name: String) -> Result<Self, WriterError> {
        fs::create_dir_all(&output_dir)?;

        info!(path = %output_dir.display(), "Output directory ensured.");

        Ok(Self {
            output_dir,
            base_name,
            analyzer: HlsAnalyzer::new(),
            current_file_writer: None,
            current_file_path: None,
            current_file_size: 0,
            current_file_start_instant: None,
            current_segment_type: None,
            current_segment_duration: 0.0,
            ts_segment_count: 0,
            mp4_segment_count: 0,
            init_segment_count: 0,
            total_segment_count: 0,
            status_callback: None,
            on_segment_open: None,
            on_segment_close: None,
            use_base_name_directly: false,
        })
    }

    /// Configure the writer to use the provided base name directly as the output filename
    /// instead of adding counters and timestamps
    pub fn use_base_name_directly(&mut self, value: bool) -> &mut Self {
        self.use_base_name_directly = value;
        self
    }

    /// Sets a callback closure that will be called with current status information.
    ///
    /// The callback receives:
    /// - `Option<&PathBuf>`: The current file path (None if no file is open)
    /// - `u64`: Current file size in bytes
    /// - `f64`: Current write rate in bytes per second
    /// - `Option<u32>`: Current duration in milliseconds (None if no duration available)
    pub fn set_status_callback<F>(&mut self, callback: F)
    where
        F: Fn(Option<&PathBuf>, u64, f64, Option<u32>) + Send + 'static,
    {
        self.status_callback = Some(Box::new(callback));
    }

    /// Sets a callback closure that will be called when a new segment is opened.
    ///
    /// The callback receives:
    /// - `&PathBuf`: Path to the file that was opened
    /// - `SegmentType`: Type of the segment that was opened
    pub fn set_on_segment_open<F>(&mut self, callback: F)
    where
        F: Fn(&PathBuf, SegmentType) + Send + 'static,
    {
        self.on_segment_open = Some(Box::new(callback));
    }

    /// Sets a callback closure that will be called when a segment is closed.
    ///
    /// The callback receives:
    /// - `&PathBuf`: Path to the file that was closed
    /// - `SegmentType`: Type of the segment that was closed
    /// - `u64`: Size of the segment in bytes
    /// - `f32`: Duration of the segment in seconds (if available)
    pub fn set_on_segment_close<F>(&mut self, callback: F)
    where
        F: Fn(&PathBuf, SegmentType, u64, f32) + Send + 'static,
    {
        self.on_segment_close = Some(Box::new(callback));
    }

    /// Calculates the current write rate in bytes per second.
    fn calculate_write_rate(&self) -> f64 {
        if let Some(start_time) = self.current_file_start_instant {
            let elapsed = start_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                return self.current_file_size as f64 / elapsed;
            }
        }
        0.0
    }

    /// Calculates the current duration of the file in milliseconds.
    fn calculate_current_duration(&self) -> Option<u32> {
        if self.current_segment_duration > 0.0 {
            Some((self.current_segment_duration * 1000.0) as u32)
        } else {
            None
        }
    }

    /// Updates the file size and calls the status callback if set.
    fn update_stats(&self) {
        if let Some(callback) = &self.status_callback {
            let rate = self.calculate_write_rate();
            let duration = self.calculate_current_duration();
            callback(
                self.current_file_path.as_ref(),
                self.current_file_size,
                rate,
                duration,
            );
        }
    }

    /// Get extension for the current segment type
    fn get_extension_for_segment_type(&self, segment_type: SegmentType) -> &'static str {
        match segment_type {
            SegmentType::Ts => "ts",
            SegmentType::M4sInit => "mp4",
            SegmentType::M4sMedia => "m4s",
            SegmentType::EndMarker => "txt", // Should not happen in practice
        }
    }

    /// Generate filename for a segment
    fn generate_filename(&self, segment_type: SegmentType) -> PathBuf {
        if self.use_base_name_directly {
            // Just append extension
            let ext = self.get_extension_for_segment_type(segment_type);
            return self.output_dir.join(format!("{}.{}", self.base_name, ext));
        }

        // More detailed filename depending on segment type and counter
        let (counter, prefix) = match segment_type {
            SegmentType::Ts => (self.ts_segment_count, "ts"),
            SegmentType::M4sInit => (self.init_segment_count, "init"),
            SegmentType::M4sMedia => (self.mp4_segment_count, "seg"),
            SegmentType::EndMarker => (0, "end"), // Should not happen in practice
        };

        let ext = self.get_extension_for_segment_type(segment_type);
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");

        self.output_dir.join(format!(
            "{}_{}_{}_{:03}.{}",
            self.base_name, prefix, timestamp, counter, ext
        ))
    }

    /// Helper method to safely interact with the current writer
    fn with_writer<F, R>(&mut self, f: F) -> Result<Option<R>, WriterError>
    where
        F: FnOnce(&mut BufWriter<fs::File>) -> Result<R, WriterError>,
    {
        if let Some(writer) = self.current_file_writer.as_mut() {
            Ok(Some(f(writer)?))
        } else {
            Ok(None)
        }
    }

    /// Close the current writer and release resources
    fn close_writer(&mut self) -> Result<(), WriterError> {
        if let Some(writer) = self.current_file_writer.as_mut() {
            writer.flush()?;
            self.current_file_writer = None;
        }
        Ok(())
    }

    /// Write the segment to disk or handle special segment types like end markers
    fn write_segment(&mut self, segment_data: &HlsData) -> Result<(), WriterError> {
        // Handle end markers as a signal to finalize current segment group
        if matches!(segment_data, HlsData::EndMarker) {
            debug!("End of playlist marker received");

            // terminate the current file writer if it exists
            self.close_writer()?;

            // Build intermediate stats for the current segment group
            match self.analyzer.build_stats() {
                Ok(stats) => {
                    info!("Segment group statistics:\n{}", stats);
                }
                Err(e) => {
                    warn!("Failed to build segment group statistics: {}", e);
                }
            }

            // Reset segment-specific counters but keep total counts
            self.current_file_path = None;
            self.current_file_size = 0;
            self.current_file_start_instant = None;
            self.current_segment_type = None;
            self.current_segment_duration = 0.0;

            // Update stats for UI
            self.update_stats();

            return Ok(());
        }

        // Analyze the segment to collect statistics
        match self.analyzer.analyze_segment(segment_data) {
            Ok(_) => debug!("Segment analyzed successfully"),
            Err(e) => warn!("Error analyzing segment: {}", e),
        }

        // Get segment type and data
        let segment_type = segment_data.segment_type();
        let segment_bytes = segment_data
            .data()
            .ok_or_else(|| WriterError::State("Segment has no data".to_string()))?;

        // Get segment duration if available
        let segment_duration = segment_data
            .media_segment()
            .map(|seg| seg.duration)
            .unwrap_or(0.0);

        // Increment appropriate counter
        match segment_type {
            SegmentType::Ts => self.ts_segment_count += 1,
            SegmentType::M4sInit => self.init_segment_count += 1,
            SegmentType::M4sMedia => self.mp4_segment_count += 1,
            SegmentType::EndMarker => {} // Should not happen - handled above
        }        // Generate output filename
        if let Some(writer) = self.current_file_writer.as_mut() {
            // keep writing to the same file if the segment type is the same
            if self.current_segment_type == Some(segment_type) {
                debug!("Appending to existing file: {:?}", self.current_file_path);
                writer.write_all(segment_bytes)?;
                writer.flush()?;
                self.current_file_size += segment_bytes.len() as u64;
                self.update_stats();
                return Ok(());
            } else {
                // Close the current file writer
                self.close_writer()?;
            }
        }

        let output_path = self.generate_filename(segment_type);
        self.current_file_path = Some(output_path.clone());
        self.current_segment_type = Some(segment_type);
        self.current_segment_duration = segment_duration;
        self.total_segment_count += 1;

        info!(
            segment_type = ?segment_type,
            path = %output_path.display(),
            size = segment_bytes.len(),
            duration = segment_duration,
            "Writing segment"
        );

        // Check if file exists
        if output_path.exists() {
            warn!(path = %output_path.display(), "Output file already exists, will be overwritten");
        }

        // Call segment open callback if set
        if let Some(callback) = &self.on_segment_open {
            callback(&output_path, segment_type);
        }

        // Reset stats for new file
        self.current_file_size = 0;
        self.current_file_start_instant = Some(Instant::now());

        // Write segment data
        let file = fs::File::create(&output_path)?;
        self.current_file_writer = Some(BufWriter::with_capacity(1024 * 1024, file));
        self.current_file_writer
            .as_mut()
            .unwrap()
            .write_all(segment_bytes)?;
        self.current_file_writer.as_mut().unwrap().flush()?;

        // Update size and stats
        self.current_file_size = segment_bytes.len() as u64;
        self.update_stats();

        // Call segment close callback if set
        if let Some(callback) = &self.on_segment_close {
            callback(
                &output_path,
                segment_type,
                self.current_file_size,
                segment_duration,
            );
        }

        Ok(())
    }

    /// Consumes the stream and writes HLS data to files
    pub fn run(
        &mut self,
        receiver: Receiver<Result<HlsData, pipeline_common::PipelineError>>,
    ) -> Result<(), WriterError> {
        let mut error: Option<Box<dyn Error + Send + Sync>> = None;

        while let Ok(result) = receiver.recv() {
            match result {
                Ok(segment_data) => {
                    self.write_segment(&segment_data)?;
                }
                Err(e) => {
                    tracing::error!(error = ?e, "Error received from pipeline stream. Writing continues.");
                    error = Some(Box::new(e));
                }
            }
        }

        // Build final stats
        match self.analyzer.build_stats() {
            Ok(stats) => {
                info!("Final HLS statistics:\n{}", stats);
            }
            Err(e) => {
                warn!("Failed to build final statistics: {}", e);
            }
        }

        info!(
            total_segments_written = self.total_segment_count,
            ts_segments = self.ts_segment_count,
            mp4_segments = self.mp4_segment_count,
            init_segments = self.init_segment_count,
            "HlsWriterTask finished writing."
        );

        if let Some(err) = error {
            tracing::error!(error = ?err, "Error occurred during writing.");
            // Return the original error wrapped in our WriterError
            return Err(WriterError::State(err.to_string()));
        }

        Ok(())
    }

    /// Gets the current download path, or None if no file is active
    pub fn current_path(&self) -> Option<&PathBuf> {
        self.current_file_path.as_ref()
    }

    /// Gets the current file size in bytes
    pub fn current_size(&self) -> u64 {
        self.current_file_size
    }

    /// Gets the current write rate in bytes per second
    pub fn write_rate(&self) -> f64 {
        self.calculate_write_rate()
    }

    /// Gets the current duration in milliseconds, or None if not available
    pub fn current_duration(&self) -> Option<u32> {
        self.calculate_current_duration()
    }

    /// Gets the total number of segments written
    pub fn total_segments_written(&self) -> u64 {
        self.total_segment_count
    }

    /// Gets the number of TS segments written
    pub fn ts_segments_written(&self) -> u32 {
        self.ts_segment_count
    }

    /// Gets the number of MP4 media segments written
    pub fn mp4_segments_written(&self) -> u32 {
        self.mp4_segment_count
    }

    /// Gets the number of MP4 initialization segments written
    pub fn init_segments_written(&self) -> u32 {
        self.init_segment_count
    }

    /// Gets a reference to the analyzer's statistics
    pub fn stats(&self) -> &crate::analyzer::HlsStats {
        &self.analyzer.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hls::M4sData;
    use m3u8_rs::MediaSegment;
    use std::sync::mpsc;

    // Helper function to create a test segment
    fn create_test_segment(segment_type: SegmentType, duration: f32) -> HlsData {
        match segment_type {
            SegmentType::Ts => {
                let mut data = vec![0u8; 188 * 10]; // 10 TS packets
                data[0] = 0x47; // TS sync byte
                data[188] = 0x47; // Next packet sync byte

                HlsData::TsData(hls::TsSegmentData {
                    segment: MediaSegment {
                        uri: "segment.ts".to_string(),
                        duration,
                        ..MediaSegment::empty()
                    },
                    data: Bytes::from(data),
                })
            }
            SegmentType::M4sInit => {
                let mut data = vec![0u8; 128];

                // Add fake 'ftyp' box
                data[0] = 0x00;
                data[1] = 0x00;
                data[2] = 0x00;
                data[3] = 0x20; // size: 32 bytes
                data[4] = b'f';
                data[5] = b't';
                data[6] = b'y';
                data[7] = b'p';

                // Add fake 'moov' box
                data[32] = 0x00;
                data[33] = 0x00;
                data[34] = 0x00;
                data[35] = 0x60; // size: 96 bytes
                data[36] = b'm';
                data[37] = b'o';
                data[38] = b'o';
                data[39] = b'v';

                HlsData::M4sData(M4sData::InitSegment(hls::M4sInitSegmentData {
                    segment: MediaSegment {
                        uri: "init.mp4".to_string(),
                        ..MediaSegment::empty()
                    },
                    data: Bytes::from(data),
                }))
            }
            SegmentType::M4sMedia => {
                let mut data = vec![0u8; 128];

                // Add fake 'moof' box
                data[0] = 0x00;
                data[1] = 0x00;
                data[2] = 0x00;
                data[3] = 0x40; // size: 64 bytes
                data[4] = b'm';
                data[5] = b'o';
                data[6] = b'o';
                data[7] = b'f';

                // Add fake 'mdat' box
                data[64] = 0x00;
                data[65] = 0x00;
                data[66] = 0x00;
                data[67] = 0x40; // size: 64 bytes
                data[68] = b'm';
                data[69] = b'd';
                data[70] = b'a';
                data[71] = b't';

                HlsData::M4sData(M4sData::Segment(hls::M4sSegmentData {
                    segment: MediaSegment {
                        uri: "segment.m4s".to_string(),
                        duration,
                        ..MediaSegment::empty()
                    },
                    data: Bytes::from(data),
                }))
            }
            SegmentType::EndMarker => HlsData::EndMarker,
        }
    }

    #[test]
    fn test_write_segment() -> Result<(), Box<dyn std::error::Error>> {
        // Create a temporary directory for test output
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path().to_owned();

        // Create writer task
        let mut writer = HlsWriterTask::new(temp_path.clone(), "test".to_string())?;

        // Create a test segment
        let segment = create_test_segment(SegmentType::Ts, 2.0);

        // Write the segment
        writer.write_segment(&segment)?;

        // Verify a segment was written
        assert_eq!(writer.ts_segments_written(), 1);
        assert_eq!(writer.total_segments_written(), 1);

        // Check that analyzer has the segment
        assert_eq!(writer.stats().ts_segment_count, 1);
        assert_eq!(writer.stats().total_duration, 2.0);

        Ok(())
    }

    #[test]
    fn test_run_with_segments() -> Result<(), Box<dyn std::error::Error>> {
        // Create a temporary directory for test output
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path().to_owned();

        // Create writer task
        let mut writer = HlsWriterTask::new(temp_path.clone(), "test".to_string())?;

        // Create channel for sending segments
        let (sender, receiver) = mpsc::channel();

        // Create test segments
        let segments = vec![
            create_test_segment(SegmentType::Ts, 2.0),
            create_test_segment(SegmentType::M4sInit, 0.0),
            create_test_segment(SegmentType::M4sMedia, 3.0),
            create_test_segment(SegmentType::EndMarker, 0.0),
        ];

        // Send segments
        for segment in segments {
            sender.send(Ok(segment))?;
        }

        // Drop sender to close the channel
        drop(sender);

        // Run the writer
        writer.run(receiver)?;

        // Verify segments were written
        assert_eq!(writer.ts_segments_written(), 1);
        assert_eq!(writer.init_segments_written(), 1);
        assert_eq!(writer.mp4_segments_written(), 1);
        assert_eq!(writer.total_segments_written(), 3); // EndMarker is not written

        // Check final stats
        assert_eq!(writer.stats().total_segment_count, 3);
        assert_eq!(writer.stats().total_duration, 5.0);

        Ok(())
    }

    #[test]
    fn test_status_callback() -> Result<(), Box<dyn std::error::Error>> {
        // Create a temporary directory for test output
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path().to_owned();

        // Create writer task
        let mut writer = HlsWriterTask::new(temp_path.clone(), "test".to_string())?;

        // Track callback invocations using an atomic counter
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = Arc::clone(&callback_count);

        writer.set_status_callback(move |path, size, _rate, _duration| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            assert!(path.is_some());
            assert!(size > 0);
        });

        // Create a test segment
        let segment = create_test_segment(SegmentType::Ts, 2.0);

        // Write the segment
        writer.write_segment(&segment)?;

        // Verify callback was called
        assert!(callback_count.load(Ordering::SeqCst) > 0);

        Ok(())
    }
}
