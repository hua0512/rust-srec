use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::mpsc::Receiver,
    time::Duration,
};

use hls::{HlsData, M4sData};
use pipeline_common::{
    FormatStrategy, OnProgress, PostWriteAction, Progress, ProtocolWriter, WriterConfig,
    WriterState, WriterTask, expand_filename_template,
};
use pipeline_common::{WriterError, progress::ProgressEvent};
use tracing::{debug, error, info};

use crate::analyzer::HlsAnalyzer;

pub struct HlsFormatStrategy {
    analyzer: HlsAnalyzer,
    current_offset: u64,
    is_finalizing: bool,
    target_duration: f32,
    on_progress: Option<OnProgress>,
}

#[derive(Debug, thiserror::Error)]
pub enum HlsStrategyError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Analyzer error: {0}")]
    Analyzer(String),
    #[error("Pipeline error: {0}")]
    Pipeline(#[from] pipeline_common::PipelineError),
}

impl HlsFormatStrategy {
    pub fn new(on_progress: Option<OnProgress>) -> Self {
        Self {
            analyzer: HlsAnalyzer::new(),
            current_offset: 0,
            is_finalizing: false,
            target_duration: 0.0,
            on_progress,
        }
    }

    fn reset(&mut self) -> Result<(), HlsStrategyError> {
        self.is_finalizing = false;
        self.analyzer.reset();
        self.current_offset = 0;
        self.target_duration = 0.0;
        Ok(())
    }

    fn update_status(&self, state: &WriterState) {
        if let Some(callback) = &self.on_progress {
            let progress = Progress {
                bytes_written: state.bytes_written_current_file,
                total_bytes: None, // HLS streams don't have a known total size
                items_processed: state.items_written_current_file as u64,
                rate: 0.0,
                duration: Some(Duration::from_secs_f32(self.target_duration)),
            };
            callback(ProgressEvent::ProgressUpdate {
                path: state.current_path.clone(),
                progress,
            });
        }
    }
}

impl FormatStrategy<HlsData> for HlsFormatStrategy {
    type Writer = BufWriter<std::fs::File>;
    type StrategyError = HlsStrategyError;

    fn create_writer(&self, path: &std::path::Path) -> Result<Self::Writer, Self::StrategyError> {
        debug!("Creating writer for path: {}", path.display());
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(BufWriter::with_capacity(1024 * 1024, file))
    }

    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &HlsData,
    ) -> Result<u64, Self::StrategyError> {
        match item {
            HlsData::TsData(ts) => {
                self.analyzer
                    .analyze_segment(item)
                    .map_err(HlsStrategyError::Analyzer)?;
                let bytes_written = ts.data.len() as u64;
                writer.write_all(&ts.data)?;
                Ok(bytes_written)
            }
            HlsData::M4sData(m4s_data) => {
                self.analyzer
                    .analyze_segment(item)
                    .map_err(HlsStrategyError::Analyzer)?;
                let bytes_written = match m4s_data {
                    M4sData::InitSegment(init) => {
                        info!("Found init segment, offset: {:?}", self.current_offset);
                        let bytes_written = init.data.len() as u64;
                        writer.write_all(&init.data)?;
                        bytes_written
                    }
                    M4sData::Segment(segment) => {
                        let bytes_written = segment.data.len() as u64;
                        writer.write_all(&segment.data)?;
                        self.target_duration = self.target_duration.max(segment.segment.duration);
                        bytes_written
                    }
                };
                self.current_offset += bytes_written;

                Ok(bytes_written)
            }
            // do nothing for end marker, it will be handled in after_item_written
            HlsData::EndMarker => Ok(0),
        }
    }

    fn should_rotate_file(&self, _config: &WriterConfig, _state: &WriterState) -> bool {
        false
    }

    fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf {
        let filename =
            expand_filename_template(&config.file_name_template, Some(state.file_sequence_number));
        let path = config.base_path.join(filename);
        let new_path = path.with_extension(&config.file_extension);
        debug!("Next file path: {}", new_path.display());
        new_path
    }

    fn on_file_open(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        if let Some(callback) = &self.on_progress {
            callback(ProgressEvent::FileOpened {
                path: path.to_path_buf(),
            });
        }
        Ok(0)
    }

    fn on_file_close(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        if self.is_finalizing {
            self.reset()?;
        }
        if let Some(callback) = &self.on_progress {
            callback(ProgressEvent::FileClosed {
                path: path.to_path_buf(),
            });
        }
        Ok(0)
    }

    fn after_item_written(
        &mut self,
        item: &HlsData,
        _bytes_written: u64,
        state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        self.update_status(state);
        if matches!(item, HlsData::EndMarker) {
            let stats = self
                .analyzer
                .build_stats()
                .map_err(HlsStrategyError::Analyzer)?;
            debug!("HLS stats: {:?}", stats);
            self.is_finalizing = true;
            Ok(PostWriteAction::Rotate)
        } else {
            Ok(PostWriteAction::None)
        }
    }
}

pub struct HlsWriter {
    writer_task: WriterTask<HlsData, HlsFormatStrategy>,
}

impl ProtocolWriter for HlsWriter {
    type Item = HlsData;
    type Stats = (usize, u32);
    type Error = WriterError<HlsStrategyError>;

    fn new(
        output_dir: PathBuf,
        base_name: String,
        extension: String,
        on_progress: Option<OnProgress>,
    ) -> Self {
        let writer_config = WriterConfig::new(output_dir, base_name, extension);
        let strategy = HlsFormatStrategy::new(on_progress);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    fn run(
        &mut self,
        receiver: Receiver<Result<HlsData, pipeline_common::PipelineError>>,
    ) -> Result<(usize, u32), WriterError<HlsStrategyError>> {
        for result in receiver.iter() {
            match result {
                Ok(hls_data) => {
                    debug!("Received HLS data: {:?}", hls_data.tag_type());
                    self.writer_task.process_item(hls_data)?;
                }
                Err(e) => {
                    tracing::error!("Error in received HLS data: {}", e);
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
