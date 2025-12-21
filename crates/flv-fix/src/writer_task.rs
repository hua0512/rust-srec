use crate::{analyzer::FlvAnalyzer, script_modifier};
use flv::{FlvData, FlvHeader, FlvWriter};
use pipeline_common::{
    FormatStrategy, PostWriteAction, WriterConfig, WriterState, expand_filename_template,
};
use std::{
    fs::OpenOptions,
    io::BufWriter,
    path::{Path, PathBuf},
    time::Instant,
};

use tracing::{Span, info};
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Error type for FLV strategy
#[derive(Debug, thiserror::Error)]
pub enum FlvStrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("FLV error: {0}")]
    Flv(#[from] flv::FlvError),
    #[error("Analysis error: {0}")]
    Analysis(String),
    #[error("Script modifier error: {0}")]
    ScriptModifier(#[from] script_modifier::ScriptModifierError),
}

/// FLV-specific format strategy implementation
pub struct FlvFormatStrategy {
    // FLV-specific state
    analyzer: FlvAnalyzer,
    pending_header: Option<FlvHeader>,
    // Internal state
    file_start_instant: Option<Instant>,
    last_header_received: bool,
    current_tag_count: u64,

    // Whether to use low-latency mode for metadata modification.
    enable_low_latency: bool,
}

impl FlvFormatStrategy {
    pub fn new(enable_low_latency: bool) -> Self {
        Self {
            analyzer: FlvAnalyzer::default(),
            pending_header: None,
            file_start_instant: None,
            last_header_received: false,
            current_tag_count: 0,
            enable_low_latency,
        }
    }

    fn calculate_duration(&self) -> u32 {
        self.analyzer.stats.calculate_duration()
    }

    fn update_status(&self, state: &WriterState) {
        // Update the current span with progress information
        let span = Span::current();
        span.pb_set_position(state.bytes_written_current_file);
        span.pb_set_message(&format!(
            "{} | {} tags | {} ms",
            state.current_path.display(),
            self.current_tag_count,
            self.calculate_duration()
        ));
    }
}

impl FormatStrategy<FlvData> for FlvFormatStrategy {
    type Writer = FlvWriter<BufWriter<std::fs::File>>;
    type StrategyError = FlvStrategyError;

    fn create_writer(&self, path: &Path) -> Result<Self::Writer, Self::StrategyError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let buf_writer = BufWriter::with_capacity(1024 * 1024, file);
        Ok(FlvWriter::new(buf_writer)?)
    }

    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &FlvData,
    ) -> Result<u64, Self::StrategyError> {
        match item {
            FlvData::Header(header) => {
                self.pending_header = Some(header.clone());
                self.last_header_received = true;
                Ok(0)
            }
            FlvData::Tag(tag) => {
                let mut bytes_written = 0;

                // If a header is pending, write it first.
                if let Some(header) = self.pending_header.take() {
                    self.analyzer
                        .analyze_header(&header)
                        .map_err(|e| FlvStrategyError::Analysis(e.to_string()))?;
                    writer.write_header(&header)?;
                    bytes_written += 13;
                }

                if self.last_header_received {
                    self.last_header_received = false;
                }

                self.current_tag_count += 1;

                self.analyzer
                    .analyze_tag(tag)
                    .map_err(|e| FlvStrategyError::Analysis(e.to_string()))?;

                writer.write_tag_f(tag)?;
                bytes_written += (11 + 4 + tag.data.len()) as u64;
                Ok(bytes_written)
            }
            FlvData::EndOfSequence(_) => {
                tracing::debug!("Received EndOfSequence, stream ending");
                Ok(0)
            }
        }
    }

    fn should_rotate_file(&self, _config: &WriterConfig, _state: &WriterState) -> bool {
        // Rotate if we've received a header and we've already written some tags to the current file.
        self.last_header_received && self.current_tag_count > 0
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
        path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        self.file_start_instant = Some(Instant::now());
        self.analyzer.reset();
        self.current_tag_count = 0;

        info!(path = %path.display(), "Opening segment");

        // Initialize the span's progress bar
        let span = Span::current();
        span.pb_set_message(&format!("Writing {}", path.display()));

        self.last_header_received = false;
        Ok(0)
    }

    fn on_file_close(
        &mut self,
        writer: &mut Self::Writer,
        path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        writer.flush()?;

        let duration = self.calculate_duration();
        let tag_count = self.current_tag_count;
        let mut analyzer = std::mem::take(&mut self.analyzer);

        if let Ok(stats) = analyzer.build_stats().cloned() {
            info!("Path : {}: {}", path.display(), &stats);
            let path_buf = path.to_path_buf();
            let enable_low_latency = self.enable_low_latency;

            // Spawn the blocking I/O operation in a separate thread.
            tokio::task::spawn_blocking(move || {
                match script_modifier::inject_stats_into_script_data(
                    &path_buf,
                    &stats,
                    enable_low_latency,
                ) {
                    Ok(_) => {
                        tracing::info!(path = %path_buf.display(), "Successfully injected stats in background task");
                    }
                    Err(e) => {
                        tracing::warn!(path = %path_buf.display(), error = ?e, "Failed to inject stats into script data section in background task");
                    }
                }

                info!(
                    path = %path_buf.display(),
                    tags = tag_count,
                    duration_ms = ?duration,
                    "Closed segment"
                );
            });
        } else {
            info!(
                path = %path.display(),
                tags = tag_count,
                duration_ms = ?duration,
                "Closed segment"
            );
        }

        // Reset the analyzer and place it back into the strategy object for the next file segment.
        analyzer.reset();
        self.analyzer = analyzer;

        Ok(0)
    }

    fn after_item_written(
        &mut self,
        _item: &FlvData,
        _bytes_written: u64,
        state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        self.update_status(state);
        if state.items_written_total.is_multiple_of(50000) {
            tracing::debug!(
                tags_written = state.items_written_total,
                "Writer progress..."
            );
        }
        Ok(PostWriteAction::None)
    }

    fn current_media_duration_secs(&self) -> f64 {
        self.calculate_duration() as f64
    }
}
