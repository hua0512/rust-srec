use crate::writer_task::FlvStrategyError;
use flv::error::FlvError;
use pipeline_common::TaskError;
use thiserror::Error;

/// Error type for the `FlvWriter`.
#[derive(Debug, Error)]
pub enum FlvWriterError {
    /// An error occurred in the underlying writer task.
    #[error("Writer task error: {0}")]
    Task(#[from] TaskError<FlvStrategyError>),

    /// An error was received from the input stream.
    #[error("Input stream error: {0}")]
    InputError(FlvError),
}

use crate::writer_task::FlvFormatStrategy;
use flv::data::FlvData;
use pipeline_common::{OnProgress, WriterConfig, WriterState, WriterTask};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// A specialized writer task for FLV data.
pub struct FlvWriter {
    writer_task: WriterTask<FlvData, FlvFormatStrategy>,
}

impl FlvWriter {
    /// Creates a new `FlvWriter`.
    pub fn new(
        output_dir: PathBuf,
        base_name: String,
        on_progress: Option<OnProgress>,
    ) -> Self {
        let writer_config = WriterConfig::new(output_dir, base_name, "flv".to_string());
        let strategy = FlvFormatStrategy::new(on_progress);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    pub fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    /// Runs the writer task, consuming FLV data from the provided receiver.
    pub fn run(
        &mut self,
        receiver: Receiver<Result<FlvData, FlvError>>,
    ) -> Result<(usize, u32), FlvWriterError> {
        for result in receiver.iter() {
            match result {
                Ok(flv_data) => {
                    self.writer_task.process_item(flv_data)?;
                }
                Err(e) => {
                    tracing::error!("Error in received FLV data: {}", e);
                    return Err(FlvWriterError::InputError(e));
                }
            }
        }
        self.writer_task.close()?;

        let final_state = self.get_state();
        let total_tags_written = final_state.items_written_total;
        let files_created = final_state.file_sequence_number;

        Ok((total_tags_written, files_created))
    }
}
