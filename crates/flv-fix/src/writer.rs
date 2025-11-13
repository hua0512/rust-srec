use crate::writer_task::FlvStrategyError;
use pipeline_common::{PipelineError, ProtocolWriter, WriterError};

use crate::writer_task::FlvFormatStrategy;
use flv::data::FlvData;
use pipeline_common::{WriterConfig, WriterState, WriterTask};
use std::collections::HashMap;
use std::sync::mpsc;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct FlvWriterConfig {
    pub enable_low_latency: bool,
}

/// A specialized writer task for FLV data.
pub struct FlvWriter {
    writer_task: WriterTask<FlvData, FlvFormatStrategy>,
}

impl ProtocolWriter for FlvWriter {
    type Item = FlvData;
    type Stats = (usize, u32);
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
        input_stream: mpsc::Receiver<Result<Self::Item, PipelineError>>,
    ) -> Result<Self::Stats, Self::Error> {
        for result in input_stream.iter() {
            match result {
                Ok(flv_data) => {
                    if let Err(e) = self.writer_task.process_item(flv_data) {
                        tracing::error!("Error processing FLV data: {}", e);
                        return Err(WriterError::TaskError(e));
                    }
                }
                Err(e) => {
                    tracing::error!("Error in received FLV data: {}", e);
                    return Err(WriterError::InputError(e.to_string()));
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
