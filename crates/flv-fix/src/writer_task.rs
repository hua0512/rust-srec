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

use std::{
    fs,
    io::{self, Read, Seek, Write},
    path::PathBuf,
};

use chrono::Local;
use flv::{data::FlvData, header::FlvHeader, tag::FlvTagData, writer::FlvWriter};
use tokio::task::spawn_blocking;
use tokio_stream::StreamExt;
use tracing::{debug, info};

use crate::{
    analyzer::FlvAnalyzer,
    pipeline::BoxStream,
    script_modifier::{self, ScriptModifierError},
};

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
    State(&'static str),
    #[error("Script modifier error: {0}")]
    ScriptModifier(#[from] ScriptModifierError),
}

/// Manages the writing of processed FLV data to output files using synchronous I/O
/// delegated via spawn_blocking.
pub struct FlvWriterTask {
    output_dir: PathBuf,
    base_name: String,
    extension: String,

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
}

impl FlvWriterTask {
    /// Creates a new writer task and ensures the output directory exists (using spawn_blocking).
    pub async fn new(output_dir: PathBuf, base_name: String) -> Result<Self, WriterError> {
        let dir_clone = output_dir.clone();
        spawn_blocking(move || fs::create_dir_all(&dir_clone)).await??; // First ? handles JoinError, second ? handles io::Error

        info!(path = %output_dir.display(), "Output directory ensured.");

        Ok(Self {
            output_dir,
            base_name,
            extension: "flv".to_string(),
            current_writer: None, // Initialized as None
            file_counter: 0,
            current_file_tag_count: 0,
            total_tag_count: 0,
            current_file_start_time: None,
            current_file_last_time: None,
            analyzer: FlvAnalyzer::new(),
            current_file_path: None, // Initialize to None
        })
    }

    /// Consumes the stream and writes FLV data to one or more files.
    pub async fn run(&mut self, stream: BoxStream<FlvData>) -> Result<(), WriterError> {
        futures::pin_mut!(stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(FlvData::Header(header)) => {
                    self.handle_header(header).await?;
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
                    let write_result = spawn_blocking(move || {
                        match &mut writer_opt {
                            Some(writer) => {
                                writer.write_tag(tag_type, data, timestamp_ms)?;
                                Ok(writer_opt) // Return the Option containing the writer
                            }
                            None => {
                                // This should ideally not happen if handle_header was called first
                                Err(WriterError::State(
                                    "Attempted write_tag with no active writer",
                                ))
                            }
                        }
                    })
                    .await?; // Handle JoinError

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

                    // Log progress periodically
                    if current_total_count % 50000 == 0 {
                        tracing::debug!(tags_written = current_total_count, "Writer progress...");
                    }
                }
                Err(e) => {
                    tracing::error!(error = ?e, "Error received from pipeline stream. Writing continues.");
                }
                #[allow(unreachable_patterns)]
                Ok(_) => {
                    tracing::warn!("Received unexpected FlvData variant during writing.");
                }
            }
        }

        self.close_current_writer().await?;

        info!(
            total_tags_written = self.total_tag_count,
            output_files_created = self.file_counter,
            "FlvWriterTask finished writing."
        );

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
    async fn handle_header(&mut self, header: FlvHeader) -> Result<(), WriterError> {
        self.close_current_writer().await?;

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
        let output_path = self.output_dir.join(format!(
            "{}_part{:03}_{}.{}",
            self.base_name,
            self.file_counter,
            Local::now().format("%Y%m%d_%H%M%S"),
            self.extension
        ));
        self.current_file_path = Some(output_path.clone()); // Store the path for later use
        let header_clone = header.clone();

        info!(path = %output_path.display(), file_num, "Creating new output file segment.");

        // Perform blocking file creation and writer initialization
        let new_writer = spawn_blocking(move || {
            let output_file = std::fs::File::create(&output_path)?;
            let buffered_writer = std::io::BufWriter::new(output_file);
            FlvWriter::with_header(buffered_writer, &header_clone)
        })
        .await??; // Handle JoinError + io::Error/FlvError

        self.current_writer = Some(new_writer);
        Ok(())
    }

    /// Closes the current file writer using spawn_blocking.
    async fn close_current_writer(&mut self) -> Result<(), WriterError> {
        if let Some(writer) = self.current_writer.take() {
            // Take ownership
            let duration_ms = self
                .current_file_last_time
                .zip(self.current_file_start_time)
                .map(|(last, first)| last.saturating_sub(first));
            let tags = self.current_file_tag_count;
            let file_num = self.file_counter;

            info!(tags, file_num, duration_ms = ?duration_ms, "Closing file segment (delegating to blocking task).");

            // Move the writer into the blocking task for closing
            spawn_blocking(move || {
                writer.close()?; // Blocking close (flushes BufWriter)
                Ok::<(), WriterError>(()) // Indicate success within the Result
            })
            .await??; // Handle JoinError + io::Error/FlvError/WriterError

            let output_path = self.current_file_path.take().unwrap();
            match self.analyzer.build_stats() {
                Ok(stats) => {
                    info!("Path : {}: {}", output_path.display(), stats);
                    // Modify the script data section by injecting stats
                    match script_modifier::inject_stats_into_script_data(&output_path, stats).await
                    {
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

            self.analyzer.reset();
        }
        Ok(())
    }

    pub fn total_tags_written(&self) -> u64 {
        self.total_tag_count
    }

    pub fn files_created(&self) -> u32 {
        self.file_counter
    }
}
