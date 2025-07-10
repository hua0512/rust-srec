//! # FLV Writer Task Module
//!
//! This module provides asynchronous functionality for writing FLV (Flash Video) data
//! to files while managing the async/sync boundary efficiently.
//!
//! ## Key Features:
//!
//! - Handles sequential FLV data writing from asynchronous streams
//! - Manages multiple output file segments with proper headers
//! - Uses Tokio's `spawn_blocking` to delegate sync I/O operations to a dedicated thread pool
//! - Tracks timestamps and maintains file state without blocking the async runtime
//!
//! ## Design Pattern:
//!
//! The module employs a "take/put-back" ownership pattern to safely move the synchronous writer
//! across thread boundaries without requiring mutex synchronization. This works because the
//! processing is sequential, ensuring only one operation accesses the writer at a time.
//!
//! ## Primary Components:
//!
//! - `FlvWriterTask`: Main struct that manages the FLV writing process
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

use crate::{
    analyzer::FlvAnalyzer,
    script_modifier::{self, ScriptModifierError},
    utils::expand_filename_template,
};
use chrono::Local;
use flv::error::FlvError;
use flv::{data::FlvData, header::FlvHeader, writer::FlvWriter};
use std::sync::mpsc::Receiver;
use std::{
    error::Error,
    fs,
    io::{self},
    path::PathBuf,
    time::Instant,
};
use tracing::{debug, info};

// Custom Error type (assuming WriterError is defined as before)
#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),
    #[error("FLV Error: {0}")]
    Flv(#[from] flv::error::FlvError),
    #[error("Task Join Error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Writer state error: {0}")]
    State(String),
    #[error("Script modifier error: {0}")]
    ScriptModifier(#[from] ScriptModifierError),
}

/// Type alias for the status callback function that provides current download statistics
pub type StatusCallback = dyn Fn(Option<&PathBuf>, u64, f64, Option<u32>) + Send + 'static;

/// Type alias for the segment open callback function
pub type SegmentOpenCallback = dyn Fn(&PathBuf, u32) + Send + 'static;

/// Type alias for the segment close callback function
pub type SegmentCloseCallback = dyn Fn(&PathBuf, u32, u64, Option<u32>) + Send + 'static;

/// Manages the writing of processed FLV data to output files using synchronous I/O
/// delegated via spawn_blocking.
pub struct FlvWriterTask {
    output_dir: PathBuf,
    base_name: String,
    extension: String,
    // New field to control whether to use base name directly
    use_base_name_directly: bool,

    // Holds the stateful, synchronous FLV writer for the current output file.
    // Since FlvWriter<BufWriter<File>> is Send (File and BufWriter are Send),
    // we can move ownership of this Option into and out of spawn_blocking closures
    // using a take/put-back pattern. This avoids needing Arc<Mutex> because
    // the stream processing loop is sequential, ensuring only one blocking
    // operation accesses the writer at a time for this task instance.
    current_writer: Option<FlvWriter<std::io::BufWriter<std::fs::File>>>,
    current_file_path: Option<PathBuf>,

    analyzer: FlvAnalyzer,

    // --- State managed outside blocking calls ---
    file_counter: u32,
    current_file_tag_count: u64,
    total_tag_count: u64,
    current_file_start_time: Option<u32>,
    current_file_last_time: Option<u32>,

    // Stats tracking
    current_file_start_instant: Option<Instant>,
    current_file_size: u64,

    // Status callback
    status_callback: Option<Box<StatusCallback>>,

    // Segment open/close callbacks
    on_segment_open: Option<Box<SegmentOpenCallback>>,
    on_segment_close: Option<Box<SegmentCloseCallback>>,
}

impl FlvWriterTask {
    /// Creates a new writer task and ensures the output directory exists (using spawn_blocking).
    pub fn new(output_dir: PathBuf, base_name: String) -> Result<Self, WriterError> {
        let dir_clone = output_dir.clone();
        fs::create_dir_all(&dir_clone)?; // handles io::Error

        info!(path = %output_dir.display(), "Output directory ensured.");

        Ok(Self {
            output_dir,
            base_name,
            extension: crate::FLV_EXTENSION.to_owned(),
            use_base_name_directly: false,
            current_writer: None, // Initialized as None
            file_counter: 0,
            current_file_tag_count: 0,
            total_tag_count: 0,
            current_file_start_time: None,
            current_file_last_time: None,
            analyzer: FlvAnalyzer::default(),
            current_file_path: None, // Initialize to None
            current_file_start_instant: None,
            current_file_size: 0,
            status_callback: None,
            on_segment_open: None,
            on_segment_close: None,
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
    /// - `u32`: File counter number (segment number)
    pub fn set_on_segment_open<F>(&mut self, callback: F)
    where
        F: Fn(&PathBuf, u32) + Send + 'static,
    {
        self.on_segment_open = Some(Box::new(callback));
    }

    /// Sets a callback closure that will be called when a segment is closed.
    ///
    /// The callback receives:
    /// - `&PathBuf`: Path to the file that was closed
    /// - `u32`: File counter number (segment number)
    /// - `u64`: Number of tags written to this segment
    /// - `Option<u32>`: Duration of the segment in milliseconds (if available)
    pub fn set_on_segment_close<F>(&mut self, callback: F)
    where
        F: Fn(&PathBuf, u32, u64, Option<u32>) + Send + 'static,
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
        match (self.current_file_start_time, self.current_file_last_time) {
            (Some(start), Some(last)) if last >= start => Some(last - start),
            _ => None, // No duration available yet
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

    /// Consumes the stream and writes FLV data to one or more files.
    pub fn run(
        &mut self,
        receiver: Receiver<Result<FlvData, FlvError>>,
    ) -> Result<(), WriterError> {
        let mut error: Option<Box<dyn Error + Send + Sync>> = None;

        while let Ok(result) = receiver.recv() {
            match result {
                Ok(FlvData::Header(header)) => {
                    self.handle_header(header)?;
                    self.current_file_size = 11 + 4; // Header + PreviousTagSize
                    self.current_file_start_instant = Some(Instant::now());
                    self.update_stats();
                }
                Ok(FlvData::Tag(tag)) => {
                    let tag_type = tag.tag_type;
                    let data = tag.data.clone();
                    let timestamp_ms = tag.timestamp_ms;

                    // Update non-blocking state immediately
                    self.update_timestamps(timestamp_ms);
                    self.total_tag_count += 1;
                    self.current_file_tag_count += 1;
                    let current_total_count = self.total_tag_count;

                    // Take ownership of the writer to move it into the blocking task
                    let mut writer_opt = self.current_writer.take();

                    // Delegate the blocking write operation
                    let write_result = match &mut writer_opt {
                        Some(writer) => {
                            writer.write_tag(tag_type, data, timestamp_ms)?;
                            Ok(writer_opt) // Return the Option containing the writer
                        }
                        None => {
                            // This should ideally not happen if handle_header was called first
                            Err(WriterError::State(
                                crate::ERROR_NO_ACTIVE_WRITER.to_owned(),
                            ))
                        }
                    };

                    // Place the writer back after the blocking operation completes
                    self.current_writer = write_result?; // Handle io::Error/FlvError/WriterError::State

                    let analyze_result = self.analyzer.analyze_tag(&tag);
                    match analyze_result {
                        Ok(stats) => {
                            tracing::trace!(?stats, "Tag analysis successful.");
                        }
                        Err(e) => {
                            tracing::error!(error = ?e, "Tag analysis failed.");
                        }
                    }

                    // Update file size from analyzer stats and trigger the status callback
                    self.current_file_size = self.analyzer.stats.file_size;
                    self.update_stats();

                    // Log progress periodically
                    if current_total_count % 50000 == 0 {
                        tracing::debug!(tags_written = current_total_count, "Writer progress...");
                    }
                }
                Err(e) => {
                    // Check if the error is related to an invalid header
                    let error_message = e.to_string();
                    if error_message.contains("invalid header")
                        || error_message.contains("Invalid header")
                    {
                        tracing::error!("Invalid FLV header detected: {}", error_message);
                        error = Some(Box::new(flv::error::FlvError::InvalidHeader));
                    } else {
                        tracing::error!(error = ?e, "Error received from pipeline stream. Writing continues.");
                        error = Some(e.into());
                    }
                }
                #[allow(unreachable_patterns)]
                Ok(_) => {
                    tracing::warn!("Received unexpected FlvData variant during writing.");
                }
            }
        }

        self.close_current_writer()?;

        info!(
            total_tags_written = self.total_tag_count,
            output_files_created = self.file_counter,
            "FlvWriterTask finished writing."
        );

        if let Some(err) = error {
            tracing::error!(error = ?err, "Error occurred during writing.");
            // Return the original error wrapped in our WriterError
            return Err(match err.downcast::<flv::error::FlvError>() {
                Ok(flv_err) => WriterError::Flv(*flv_err),
                Err(err) => WriterError::State(err.to_string()),
            });
        }

        Ok(())
    }

    /// Updates timestamp tracking (non-blocking).
    fn update_timestamps(&mut self, ts: u32) {
        if self.current_file_start_time.is_none() {
            self.current_file_start_time = Some(ts);
        }
        self.current_file_last_time = Some(ts);
    }

    /// Handles receiving an FLV Header, closing the previous file and starting a new one.
    fn handle_header(&mut self, header: FlvHeader) -> Result<(), WriterError> {
        self.close_current_writer()?;

        // Reset non-blocking state
        self.current_file_tag_count = 0;
        self.current_file_start_time = None;
        self.current_file_last_time = None;
        self.file_counter += 1;
        let file_num = self.file_counter;
        match self.analyzer.analyze_header(&header) {
            Ok(_) => {
                tracing::debug!(file_num, "Header analysis successful.");
            }
            Err(e) => {
                tracing::error!(file_num, error = ?e, "Header analysis failed.");
            }
        }

        // Prepare data for blocking task
        let output_path = if self.use_base_name_directly {
            // Use base name directly as filename
            let file_name = expand_filename_template(&self.base_name, Some(self.file_counter));
            self.output_dir
                .join(format!("{}.{}", file_name, self.extension))
        } else {
            // Use traditional naming with part numbers and timestamps
            self.output_dir.join(format!(
                "{}_p{:03}_{}.{}",
                self.base_name,
                self.file_counter,
                Local::now().format("%Y%m%d_%H%M%S"),
                self.extension
            ))
        };

        self.current_file_path = Some(output_path.clone()); // Store the path for later use
        let header_clone = header.clone();

        info!(segment= %file_num, path = %output_path.display(), "Opening: ");

        // Reset stats for new file
        self.current_file_size = 0;
        self.current_file_start_instant = Some(Instant::now());

        // Check if the output file already exists
        if output_path.exists() {
            tracing::warn!(path = %output_path.display(), "Output file already exists, will be overwritten");
        }
        let output_file = std::fs::File::create(&output_path)?;
        let buffered_writer = std::io::BufWriter::with_capacity(1024 * 1024, output_file);
        let new_writer = FlvWriter::with_header(buffered_writer, &header_clone)?;

        self.current_writer = Some(new_writer);

        // Update stats after file creation
        self.update_stats();

        // Call segment open callback if set
        if let Some(callback) = &self.on_segment_open {
            if let Some(path) = &self.current_file_path {
                callback(path, self.file_counter);
            }
        }

        Ok(())
    }

    /// Closes the current file writer using spawn_blocking.
    fn close_current_writer(&mut self) -> Result<(), WriterError> {
        if let Some(writer) = self.current_writer.take() {
            // Take ownership
            let duration_ms = self
                .current_file_last_time
                .zip(self.current_file_start_time)
                .map(|(last, first)| last.saturating_sub(first));
            let tags = self.current_file_tag_count;
            let file_num = self.file_counter;

            // Move the writer into the blocking task for closing
            writer.close()?; // Blocking close (flushes BufWriter)

            info!(
                segment = %file_num,
                path = %self.current_file_path.as_ref().unwrap().display(),
                tags,
                duration_ms,
                "Closed:"
            );

            // Call segment close callback if set
            if let Some(callback) = &self.on_segment_close {
                if let Some(path) = &self.current_file_path {
                    callback(path, file_num, tags, duration_ms);
                }
            }

            // Final stats update before completing file
            self.update_stats();

            let output_path = self.current_file_path.take().unwrap();
            match self.analyzer.build_stats() {
                Ok(stats) => {
                    info!("Path : {}: {}", output_path.display(), stats);
                    // Modify the script data section by injecting stats
                    match script_modifier::inject_stats_into_script_data(&output_path, stats) {
                        Ok(_) => {
                            debug!("Successfully injected stats into script data section.");
                        }
                        Err(e) => {
                            tracing::error!(path = %output_path.display(), error = ?e, "Failed to inject stats into script data section.");
                            // Continue processing despite injection failure
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(file_num, error = ?e, "Failed to build stats.");
                }
            }

            // Reset file stats after closing
            self.current_file_size = 0;
            self.current_file_start_instant = None;

            // Update status with no current file
            self.update_stats();

            self.analyzer.reset();
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

    pub fn total_tags_written(&self) -> u64 {
        self.total_tag_count
    }

    pub fn files_created(&self) -> u32 {
        self.file_counter
    }
}
