use crate::{analyzer::FlvAnalyzer, script_modifier};
use flv::{FlvData, FlvHeader, FlvWriter};
use pipeline_common::{
    FormatStrategy, PostWriteAction, WriterConfig, WriterState, expand_filename_template,
};
use std::{
    fs::OpenOptions,
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
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
    last_status_update: Option<Instant>,
    last_status_bytes: u64,

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
            last_status_update: None,
            last_status_bytes: 0,
            enable_low_latency,
        }
    }

    fn calculate_duration(&self) -> u32 {
        self.analyzer.stats.calculate_duration()
    }

    fn should_update_status(&mut self, state: &WriterState) -> bool {
        const MIN_UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
        const MIN_BYTES_DELTA: u64 = 512 * 1024; // 512KiB

        let now = Instant::now();
        let last = self.last_status_update.get_or_insert(now);

        let time_due = now.duration_since(*last) >= MIN_UPDATE_INTERVAL;
        let bytes_due = state
            .bytes_written_current_file
            .saturating_sub(self.last_status_bytes)
            >= MIN_BYTES_DELTA;

        if time_due || bytes_due {
            *last = now;
            self.last_status_bytes = state.bytes_written_current_file;
            true
        } else {
            false
        }
    }

    fn update_status(&self, state: &WriterState) {
        // Update the current span with progress information
        let span = Span::current();
        span.pb_set_position(state.bytes_written_current_file);
        span.pb_set_message(&format!(
            "{} | {} tags | {}s",
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

        let extension = &config.file_extension;
        let template_has_index = config.file_name_template.contains("%i");

        let mut file_name = expand_filename_template(&config.file_name_template, Some(sequence));
        let mut candidate = config.base_path.join(format!("{file_name}.{extension}"));

        if !candidate.exists() {
            return candidate;
        }

        // Collision avoidance:
        // If the template doesn't include `%i` (sequence index), multiple segments opened within the
        // same second can resolve to the same filename and overwrite/truncate earlier segments.
        if !template_has_index {
            file_name = format!("{file_name}-{sequence:03}");
            candidate = config.base_path.join(format!("{file_name}.{extension}"));

            if !candidate.exists() {
                return candidate;
            }
        }

        for dup in 1u32..=9999 {
            let name = format!("{file_name}-dup{dup:04}");
            let path = config.base_path.join(format!("{name}.{extension}"));
            if !path.exists() {
                return path;
            }
        }

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        config
            .base_path
            .join(format!("{file_name}-dup{nanos}.{extension}"))
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
        self.last_status_update = None;
        self.last_status_bytes = 0;

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

        let duration_secs = self.calculate_duration();
        let tag_count = self.current_tag_count;
        let mut analyzer = std::mem::take(&mut self.analyzer);

        if let Ok(stats) = analyzer.build_stats().cloned() {
            info!("Path : {}: {}", path.display(), &stats);
            let path_buf = path.to_path_buf();
            let enable_low_latency = self.enable_low_latency;

            let task = move || {
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
                    duration_secs = ?duration_secs,
                    "Closed segment"
                );
            };

            // Prefer tokio's blocking pool when available, otherwise fall back to a plain thread.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn_blocking(task);
            } else {
                std::thread::spawn(task);
            }
        } else {
            info!(
                path = %path.display(),
                tags = tag_count,
                duration_secs = ?duration_secs,
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
        if self.should_update_status(state) {
            self.update_status(state);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "flv-fix-{prefix}-pid{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn next_file_path_avoids_collisions_when_template_has_no_index() {
        let base_path = make_temp_dir("collision");
        let config = WriterConfig::new(
            base_path.clone(),
            "same-name".to_string(),
            "flv".to_string(),
        );
        let strategy = FlvFormatStrategy::new(true);

        let state = WriterState {
            file_sequence_number: 0,
            ..Default::default()
        };

        let colliding = base_path.join("same-name.flv");
        std::fs::write(&colliding, b"existing").unwrap();

        let candidate = strategy.next_file_path(&config, &state);
        assert_ne!(candidate, colliding);
        assert_eq!(candidate, base_path.join("same-name-000.flv"));

        std::fs::write(&candidate, b"existing2").unwrap();
        let candidate2 = strategy.next_file_path(&config, &state);
        assert_ne!(candidate2, candidate);
        assert_eq!(candidate2, base_path.join("same-name-000-dup0001.flv"));

        let _ = std::fs::remove_dir_all(base_path);
    }
}
