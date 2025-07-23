use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Arc, mpsc::Receiver},
};

use hls::{HlsData, M4sData, SegmentType, segment::SegmentData};
use pipeline_common::{
    FormatStrategy, PostWriteAction, TaskError, WriterConfig, WriterState, WriterTask,
    expand_filename_template,
};
use thiserror::Error;
use tracing::{debug, error, info};

use crate::analyzer::HlsAnalyzer;

pub type StatusCallback =
    Arc<dyn Fn(Option<&PathBuf>, u64, f64, Option<u32>) + Send + Sync + 'static>;

#[derive(Debug, Clone)]
struct SegmentInfo {
    duration: f32,
    offset: u64,
    size: u64,
}

pub struct HlsFormatStrategy {
    analyzer: HlsAnalyzer,
    segment_info: Vec<SegmentInfo>,
    current_offset: u64,
    is_finalizing: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HlsStrategyError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Task Join Error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Analyzer error: {0}")]
    Analyzer(String),
    #[error("Pipeline error: {0}")]
    Pipeline(#[from] pipeline_common::PipelineError),
}

impl HlsFormatStrategy {
    pub fn new(status_callback: Option<StatusCallback>) -> Self {
        Self {
            analyzer: HlsAnalyzer::new(),
            segment_info: Vec::new(),
            current_offset: 0,
            is_finalizing: false,
        }
    }

    fn write_playlist(&self, path: &PathBuf, config: &WriterConfig) -> Result<(), std::io::Error> {
        let media_filename = match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Could not get media filename from path",
                ));
            }
        };
        let playlist_path = path.with_extension("m3u8");

        info!("Writing playlist to: {}", playlist_path.display());

        let mut file = File::create(playlist_path)?;
        writeln!(file, "#EXTM3U")?;
        writeln!(file, "#EXT-X-VERSION:7")?;
        let target_duration = self
            .segment_info
            .iter()
            .map(|s| s.duration)
            .fold(0.0, f32::max)
            .ceil() as u32;
        writeln!(file, "#EXT-X-TARGETDURATION:{}", target_duration)?;
        writeln!(file, "#EXT-X-MEDIA-SEQUENCE:0")?;
        writeln!(file, "#EXT-X-PLAYLIST-TYPE:VOD")?;

        // Write the init segment info
        if let Some(init_segment) = self.segment_info.first() {
            writeln!(
                file,
                "#EXT-X-MAP:URI=\"{}\",BYTERANGE=\"{}@{}\"",
                media_filename, init_segment.size, init_segment.offset
            )?;
        }

        // Write media segments
        for segment in self.segment_info.iter().skip(1) {
            writeln!(file, "#EXTINF:{:.3},", segment.duration)?;
            writeln!(file, "#EXT-X-BYTERANGE:{}@{}", segment.size, segment.offset)?;
            writeln!(file, "{}", media_filename)?;
        }

        writeln!(file, "#EXT-X-ENDLIST")?;
        Ok(())
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
                let (bytes_written, duration) = match m4s_data {
                    M4sData::InitSegment(init) => {
                        info!("Found init segment, offset: {:?}", self.current_offset);
                        let bytes_written = init.data.len() as u64;
                        writer.write_all(&init.data)?;
                        (bytes_written, 0.0)
                    }
                    M4sData::Segment(segment) => {
                        let bytes_written = segment.data.len() as u64;
                        writer.write_all(&segment.data)?;
                        (bytes_written, segment.segment.duration)
                    }
                };

                self.segment_info.push(SegmentInfo {
                    duration,
                    offset: self.current_offset,
                    size: bytes_written,
                });
                self.current_offset += bytes_written;

                Ok(bytes_written)
            }
            // do nothing for end marker, it will be handled in after_item_written
            HlsData::EndMarker => Ok(0),
        }
    }

    fn should_rotate_file(&self, config: &WriterConfig, state: &WriterState) -> bool {
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
        writer: &mut Self::Writer,
        path: &std::path::Path,
        config: &WriterConfig,
        state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        Ok(0)
    }

    fn on_file_close(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        if self.is_finalizing {
            if let Err(e) = self.write_playlist(&path.to_path_buf(), config) {
                error!("Failed to write playlist: {}", e);
            }
            // Reset state after writing playlist
            self.is_finalizing = false;
            self.analyzer.reset();
            self.segment_info.clear();
            self.current_offset = 0;
        }
        Ok(0)
    }

    fn after_item_written(
        &mut self,
        item: &HlsData,
        _bytes_written: u64,
        _state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
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

/// Error type for the `HlsWriter`.
#[derive(Debug, Error)]
pub enum HlsWriterError {
    /// An error occurred in the underlying writer task.
    #[error("Writer task error: {0}")]
    Task(#[from] TaskError<HlsStrategyError>),

    /// An error was received from the input stream.
    #[error("Input stream error: {0}")]
    InputError(pipeline_common::PipelineError),
}

pub struct HlsWriter {
    writer_task: WriterTask<HlsData, HlsFormatStrategy>,
}

impl HlsWriter {
    pub fn new(output_dir: PathBuf, base_name: String, extension: String) -> Self {
        let writer_config = WriterConfig::new(output_dir, base_name, extension);
        let strategy = HlsFormatStrategy::new(None);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    pub fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    pub fn run(
        &mut self,
        receiver: Receiver<Result<HlsData, pipeline_common::PipelineError>>,
    ) -> Result<(usize, u32), HlsWriterError> {
        for result in receiver.iter() {
            match result {
                Ok(hls_data) => {
                    debug!("Received HLS data: {:?}", hls_data.tag_type());
                    self.writer_task.process_item(hls_data)?;
                }
                Err(e) => {
                    tracing::error!("Error in received HLS data: {}", e);
                    return Err(HlsWriterError::InputError(e));
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
