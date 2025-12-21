use crate::writer_task::FlvStrategyError;
use pipeline_common::{PipelineError, ProgressConfig, ProtocolWriter, WriterError, WriterProgress};

use crate::writer_task::FlvFormatStrategy;
use flv::data::FlvData;
use pipeline_common::{WriterConfig, WriterState, WriterTask};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct FlvWriterConfig {
    pub enable_low_latency: bool,
}

/// A specialized writer task for FLV data.
pub struct FlvWriter {
    writer_task: WriterTask<FlvData, FlvFormatStrategy>,
}

impl FlvWriter {
    /// Set a callback to be invoked when a new segment starts recording.
    ///
    /// The callback receives the file path and sequence number (0-based).
    /// This is useful for emitting `SegmentEvent::SegmentStarted` notifications.
    pub fn set_on_segment_start_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_open_callback(callback);
    }

    /// Set a callback to be invoked when a segment is completed.
    ///
    /// The callback receives the file path, sequence number (0-based), duration in seconds, and size in bytes.
    /// This callback provides the segment's media duration for tracking purposes.
    pub fn set_on_segment_complete_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32, f64, u64) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_close_callback(callback);
    }

    /// Set a progress callback with default intervals (1MB bytes, 1000ms time).
    ///
    /// The callback receives a `WriterProgress` struct containing metrics about
    /// bytes written, items processed, media duration, and performance.
    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task.set_progress_callback(callback);
    }

    /// Set a progress callback with custom intervals.
    ///
    /// The callback receives a `WriterProgress` struct containing metrics about
    /// bytes written, items processed, media duration, and performance.
    pub fn set_progress_callback_with_config<F>(&mut self, callback: F, config: ProgressConfig)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task
            .set_progress_callback_with_config(callback, config);
    }

    /// Get the total media duration in seconds across all files.
    pub fn media_duration_secs(&self) -> f64 {
        self.writer_task.get_state().media_duration_secs_total
    }
}

impl ProtocolWriter for FlvWriter {
    type Item = FlvData;
    type Stats = (usize, u32, u64, f64);
    type Error = WriterError<FlvStrategyError>;

    fn new(
        output_dir: PathBuf,
        base_name: String,
        _extension: String,
        extras: Option<HashMap<String, String>>,
    ) -> Self {
        let writer_config = WriterConfig::new(output_dir, base_name, "flv".to_string());
        let enable_low_latency = extras
            .and_then(|extras| extras.get("enable_low_latency").map(|v| v == "true"))
            .unwrap_or(true);

        let strategy = FlvFormatStrategy::new(enable_low_latency);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    fn run(
        &mut self,
        mut input_stream: tokio::sync::mpsc::Receiver<Result<Self::Item, PipelineError>>,
    ) -> Result<Self::Stats, Self::Error> {
        while let Some(result) = input_stream.blocking_recv() {
            match result {
                Ok(flv_data) => {
                    if let Err(e) = self.writer_task.process_item(flv_data) {
                        tracing::error!("Error processing FLV data: {}", e);
                        return Err(WriterError::TaskError(e));
                    }
                }
                Err(e) => {
                    tracing::error!("Error in received FLV data: {}", e);
                    if let Err(close_err) = self.writer_task.close() {
                        tracing::error!(
                            "Failed to close writer task after input error: {}",
                            close_err
                        );
                    }
                    return Err(WriterError::InputError(e.to_string()));
                }
            }
        }
        self.writer_task.close()?;

        let final_state = self.get_state();
        let total_tags_written = final_state.items_written_total;
        let files_created = final_state.file_sequence_number;
        let total_bytes_written = final_state.bytes_written_total;
        let total_duration_secs = final_state.media_duration_secs_total;

        Ok((
            total_tags_written,
            files_created,
            total_bytes_written,
            total_duration_secs,
        ))
    }
}
