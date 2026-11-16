use std::path::Path;

use pipeline_common::{
    ProgressConfig, ProtocolWriter, SplitReason, WriterCallbacks, WriterError, WriterProgress,
    WriterStats,
};

use crate::writer_task::{FlvFormatStrategy, FlvWriterConfig};
use flv::data::FlvData;
use pipeline_common::{WriterConfig, WriterState, WriterTask};

/// A specialized writer task for FLV data.
pub struct FlvWriter {
    writer_task: WriterTask<FlvData, FlvFormatStrategy>,
}

impl FlvWriter {
    pub fn new(config: FlvWriterConfig) -> Self {
        let writer_config =
            WriterConfig::new(config.output_dir, config.base_name, "flv".to_string());
        let strategy = FlvFormatStrategy::new();
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    /// Set a progress callback with custom intervals.
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

impl WriterCallbacks for FlvWriter {
    fn set_on_segment_start_callback<F>(&mut self, callback: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_open_callback(callback);
    }

    fn set_on_segment_complete_callback<F>(&mut self, callback: F)
    where
        F: Fn(&Path, u32, f64, u64, Option<&SplitReason>) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_close_callback(callback);
    }

    fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task.set_progress_callback(callback);
    }
}

impl ProtocolWriter for FlvWriter {
    type Item = FlvData;

    fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    fn run(
        &mut self,
        input: pipeline_common::PipelineReceiver<Self::Item>,
    ) -> Result<WriterStats, WriterError> {
        self.writer_task.run_from_channel(input, |_, _| true)
    }
}
