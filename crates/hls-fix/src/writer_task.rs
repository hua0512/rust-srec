use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
};

use hls::{HlsData, M4sData};
use pipeline_common::{
    FormatStrategy, PipelineError, PostWriteAction, ProgressConfig, ProtocolWriter, WriterConfig,
    WriterError, WriterProgress, WriterState, WriterStats, WriterTask, expand_filename_template,
};

use tracing::{Span, debug, info};
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::analyzer::HlsAnalyzer;

pub struct HlsFormatStrategy {
    analyzer: HlsAnalyzer,
    current_offset: u64,
    target_duration: f32,
    max_file_size: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum HlsStrategyError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Analyzer error: {0}")]
    Analyzer(String),
}

impl HlsFormatStrategy {
    pub fn new(max_file_size: Option<u64>) -> Self {
        Self {
            analyzer: HlsAnalyzer::new(),
            current_offset: 0,
            target_duration: 0.0,
            max_file_size,
        }
    }

    fn reset_for_new_file(&mut self) -> Result<(), HlsStrategyError> {
        self.analyzer.reset();
        self.current_offset = 0;
        self.target_duration = 0.0;
        Ok(())
    }

    fn update_status(&self, state: &WriterState) {
        // Update the current span with progress information
        let span = Span::current();
        span.pb_set_position(state.bytes_written_current_file);
        span.pb_set_message(&format!(
            "{} | {} segments | {:.1}s",
            state.current_path.display(),
            state.items_written_current_file,
            self.target_duration
        ));
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
                // Accumulate TS segment duration
                self.target_duration += ts.segment.duration;
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
                        self.target_duration += segment.segment.duration;
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

    fn should_rotate_file(&self, _config: &WriterConfig, state: &WriterState) -> bool {
        let Some(max_size) = self.max_file_size else {
            return false;
        };
        if max_size == 0 {
            return false;
        }

        // Rotate before writing the next item once we have at least one item in the current file.
        // This avoids creating empty files when a rotation is requested before any payload is written.
        state.items_written_current_file > 0 && state.bytes_written_current_file >= max_size
    }

    fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf {
        let sequence = state.file_sequence_number;

        let file_name = expand_filename_template(&config.file_name_template, Some(sequence));
        config
            .base_path
            .join(format!("{}.{}", file_name, config.file_extension))
    }

    fn on_file_open(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        self.reset_for_new_file()?;

        info!(path = %path.display(), "Opening segment");

        // Initialize the span's progress bar
        let span = Span::current();
        span.pb_set_message(&format!("Writing {}", path.display()));

        // Set progress bar length from max_file_size if available
        if let Some(max_size) = self.max_file_size {
            span.pb_set_length(max_size);
        }

        Ok(0)
    }

    fn on_file_close(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        let items_written = state.items_written_current_file;
        let duration_secs = self.target_duration;

        info!(
            path = %path.display(),
            items = items_written,
            duration_secs = ?duration_secs,
            "Closed segment"
        );

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
            // If an end marker arrives before any real payload, don't rotate.
            // This prevents creating empty files if the stream begins with a boundary marker.
            if state.items_written_current_file <= 1 {
                return Ok(PostWriteAction::None);
            }

            let stats = self
                .analyzer
                .build_stats()
                .map_err(HlsStrategyError::Analyzer)?;
            debug!("HLS stats: {:?}", stats);
            Ok(PostWriteAction::Rotate)
        } else {
            Ok(PostWriteAction::None)
        }
    }

    fn current_media_duration_secs(&self) -> f64 {
        self.target_duration as f64
    }
}

/// Typed configuration for HLS writer.
pub struct HlsWriterConfig {
    pub output_dir: PathBuf,
    pub base_name: String,
    pub extension: String,
    pub max_file_size: Option<u64>,
}

pub struct HlsWriter {
    writer_task: WriterTask<HlsData, HlsFormatStrategy>,
}

impl HlsWriter {
    pub fn new(config: HlsWriterConfig) -> Self {
        let writer_config =
            WriterConfig::new(config.output_dir, config.base_name, config.extension);
        let strategy = HlsFormatStrategy::new(config.max_file_size);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    /// Set a callback to be invoked when a new segment starts recording.
    pub fn set_on_segment_start_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_open_callback(callback);
    }

    /// Set a callback to be invoked when a segment is completed.
    pub fn set_on_segment_complete_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32, f64, u64) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_close_callback(callback);
    }

    /// Set a progress callback with default intervals (1MB bytes, 1000ms time).
    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task.set_progress_callback(callback);
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

impl ProtocolWriter for HlsWriter {
    type Item = HlsData;

    fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    fn run(
        &mut self,
        input: tokio::sync::mpsc::Receiver<Result<HlsData, PipelineError>>,
    ) -> Result<WriterStats, WriterError> {
        let mut saw_payload = false;
        self.writer_task.run_from_channel(input, |item, _state| {
            if !saw_payload && matches!(item, HlsData::EndMarker) {
                return false;
            }
            saw_payload |= !matches!(item, HlsData::EndMarker);
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::PipelineError;

    #[test]
    fn rotates_on_max_file_size_between_items() {
        let tempdir = tempfile::tempdir().expect("create temp dir");

        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: tempdir.path().to_path_buf(),
            base_name: "test-%i".to_string(),
            extension: "ts".to_string(),
            max_file_size: Some(15),
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<HlsData, PipelineError>>(16);

        let handle = std::thread::spawn(move || writer.run(rx));

        let seg = |bytes: &'static [u8]| {
            Ok(HlsData::ts(
                MediaSegment {
                    duration: 1.0,
                    ..MediaSegment::empty()
                },
                Bytes::from_static(bytes),
            ))
        };

        tx.blocking_send(seg(&[0u8; 10])).unwrap();
        tx.blocking_send(seg(&[1u8; 10])).unwrap();
        tx.blocking_send(seg(&[2u8; 10])).unwrap();
        drop(tx);

        let stats = handle
            .join()
            .expect("writer thread join")
            .expect("writer ok");

        assert_eq!(stats.files_created, 2);

        let file_count = std::fs::read_dir(tempdir.path())
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "ts"))
            .count();
        assert_eq!(file_count, 2);
    }

    #[test]
    fn ignores_leading_end_markers() {
        let tempdir = tempfile::tempdir().expect("create temp dir");

        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: tempdir.path().to_path_buf(),
            base_name: "test-%i".to_string(),
            extension: "ts".to_string(),
            max_file_size: None,
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<HlsData, PipelineError>>(16);

        let handle = std::thread::spawn(move || writer.run(rx));

        tx.blocking_send(Ok(HlsData::EndMarker)).unwrap();
        tx.blocking_send(Ok(HlsData::EndMarker)).unwrap();
        drop(tx);

        let stats = handle
            .join()
            .expect("writer thread join")
            .expect("writer ok");
        assert_eq!(stats.files_created, 0);
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(stats.duration_secs, 0.0);

        let file_count = std::fs::read_dir(tempdir.path())
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "ts"))
            .count();
        assert_eq!(file_count, 0);
    }
}
