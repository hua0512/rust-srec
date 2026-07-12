use crate::{
    amf::{
        builder::{FixedSizeMetadataError, OnMetaDataBuilder},
        model::AmfScriptData,
    },
    analyzer::{AnalyzerError, FlvAnalyzer, FlvStats},
};
use bytes::Bytes;
use flv::{FlvData, FlvHeader, FlvWriter, script::ScriptData};
use pipeline_common::split_reason::SplitReason;
use pipeline_common::{
    FormatStrategy, PostWriteAction, WriterConfig, WriterState, expand_filename_template,
};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Seek, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use tracing::{Span, info};
use tracing_indicatif::span_ext::IndicatifSpanExt;

const METADATA_PATCH_RESERVATION_BYTES: usize = 256;

/// Error type for FLV strategy
#[derive(Debug, thiserror::Error)]
pub enum FlvStrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("FLV error: {0}")]
    Flv(#[from] flv::FlvError),
    #[error("Analysis error: {0}")]
    Analysis(#[from] AnalyzerError),
    #[error("Fixed-size metadata error: {0}")]
    FixedSizeMetadata(#[from] FixedSizeMetadataError),
}

/// Typed configuration for FLV writer.
pub struct FlvWriterConfig {
    pub output_dir: PathBuf,
    pub base_name: String,
    /// Retained for configuration compatibility; metadata patching is always layout-stable.
    pub enable_low_latency: bool,
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
    /// The most recent split reason received, if any.
    last_split_reason: Option<SplitReason>,
    metadata_patch: Option<MetadataPatch>,
}

struct MetadataPatch {
    payload_offset: u64,
    payload_size: usize,
    model: AmfScriptData,
    include_keyframes: bool,
}

impl FlvFormatStrategy {
    pub fn new(_enable_low_latency: bool) -> Self {
        Self {
            analyzer: FlvAnalyzer::default(),
            pending_header: None,
            file_start_instant: None,
            last_header_received: false,
            current_tag_count: 0,
            last_status_update: None,
            last_status_bytes: 0,
            last_split_reason: None,
            metadata_patch: None,
        }
    }

    fn calculate_duration(&self) -> u32 {
        self.analyzer.stats.calculate_duration()
    }

    /// Returns the most recently received split reason, if any.
    pub fn last_split_reason(&self) -> Option<&SplitReason> {
        self.last_split_reason.as_ref()
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

    fn prepare_metadata_patch(
        &mut self,
        tag: &flv::FlvTag,
        tag_start: u64,
    ) -> Result<Option<flv::FlvTag>, FlvStrategyError> {
        if self.metadata_patch.is_some() || !tag.is_script_tag() || tag.is_filtered() {
            return Ok(None);
        }

        let mut cursor = std::io::Cursor::new(tag.data().clone());
        let Ok(script) = ScriptData::demux(&mut cursor) else {
            return Ok(None);
        };
        if script.name != crate::AMF0_ON_METADATA {
            return Ok(None);
        }
        let Some(properties) = script
            .data
            .first()
            .and_then(amf0::Amf0Value::as_object_properties)
        else {
            return Ok(None);
        };
        let Ok(model) = AmfScriptData::from_amf_object_ref(properties) else {
            return Ok(None);
        };

        let include_keyframes = model.spacer_size.is_some() || model.keyframes.is_some();
        let prepared_tag = if model.spacer_size.is_none() {
            let builder = OnMetaDataBuilder::from_script_data(model.clone());
            let (canonical, _) = builder
                .clone()
                .build_bytes(0, false)
                .map_err(FixedSizeMetadataError::from)?;
            let reserved =
                builder.build_fixed_size(canonical.len() + METADATA_PATCH_RESERVATION_BYTES)?;
            Some(flv::FlvTag::new(
                tag.timestamp_ms,
                tag.stream_id,
                tag.tag_type(),
                tag.is_filtered(),
                Bytes::from(reserved.bytes),
            ))
        } else {
            None
        };
        let payload_size = prepared_tag
            .as_ref()
            .map_or_else(|| tag.data().len(), |prepared| prepared.data().len());

        self.metadata_patch = Some(MetadataPatch {
            payload_offset: tag_start + flv::framing::TAG_HEADER_SIZE as u64,
            payload_size,
            model,
            include_keyframes,
        });
        Ok(prepared_tag)
    }

    fn build_final_metadata(
        patch: MetadataPatch,
        stats: &FlvStats,
    ) -> Result<crate::amf::builder::FixedSizeMetadata, FixedSizeMetadataError> {
        let include_keyframes = patch.include_keyframes;
        let mut builder = OnMetaDataBuilder::from_script_data(patch.model).with_stats(stats);
        if include_keyframes && let Some(video_stats) = &stats.video_stats {
            let (times, filepositions) = video_stats
                .keyframes
                .iter()
                .map(|keyframe| (keyframe.timestamp_s, keyframe.file_position))
                .unzip();
            builder = builder.with_final_keyframes(times, filepositions);
        }
        builder.build_fixed_size(patch.payload_size)
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
                        .map_err(FlvStrategyError::Analysis)?;
                    writer.write_header(&header)?;
                    bytes_written += 13;
                }

                if self.last_header_received {
                    self.last_header_received = false;
                }

                self.current_tag_count += 1;

                let tag_start = self.analyzer.stats.file_size;
                let prepared_tag = self.prepare_metadata_patch(tag, tag_start)?;
                let tag = prepared_tag.as_ref().unwrap_or(tag);

                self.analyzer
                    .analyze_tag(tag)
                    .map_err(FlvStrategyError::Analysis)?;

                writer.write_tag_f(tag)?;
                bytes_written += (11 + 4 + tag.data().len()) as u64;
                Ok(bytes_written)
            }
            FlvData::Split(reason) => {
                self.last_split_reason = Some(reason.clone());
                Ok(0)
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
        let file_name = expand_filename_template(&config.file_name_template, Some(sequence));
        config.base_path.join(format!("{file_name}.{extension}"))
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
        self.last_split_reason = None;
        self.metadata_patch = None;

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
            if let Some(patch) = self.metadata_patch.take() {
                let payload_offset = patch.payload_offset;
                match Self::build_final_metadata(patch, &stats) {
                    Ok(metadata) => {
                        if metadata.truncated {
                            tracing::warn!(
                                path = %path.display(),
                                keyframes_written = metadata.keyframes_written,
                                "Truncated FLV keyframe index to preserve metadata layout"
                            );
                        }
                        writer
                            .writer
                            .seek(std::io::SeekFrom::Start(payload_offset))?;
                        writer.writer.write_all(&metadata.bytes)?;
                        writer.writer.flush()?;
                    }
                    Err(FixedSizeMetadataError::TooLarge { target, minimum }) => {
                        tracing::warn!(
                            path = %path.display(),
                            target,
                            minimum,
                            "Metadata reservation is too small; leaving the script tag unchanged"
                        );
                    }
                    Err(error) => return Err(error.into()),
                }
            }

            info!(
                path = %path.display(),
                tags = tag_count,
                duration_secs = ?duration_secs,
                "Closed segment"
            );
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

    fn close_context(&self) -> Option<SplitReason> {
        self.last_split_reason.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Seek,
        sync::{Arc, Mutex},
    };

    use amf0::Amf0Value;
    use bytes::Bytes;
    use flv::{FlvTag, FlvTagType, parser::FlvParser, script::ScriptData};
    use pipeline_common::{PipelineError, ProtocolWriter};

    use super::*;
    use crate::writer::FlvWriter as RecordingWriter;

    #[test]
    fn writer_patches_reserved_metadata_before_run_returns() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut writer = RecordingWriter::new(FlvWriterConfig {
            output_dir: tempdir.path().to_path_buf(),
            base_name: "segment-%i".to_string(),
            enable_low_latency: true,
        });
        let opened_path = Arc::new(Mutex::new(None));
        let callback_path = Arc::clone(&opened_path);
        writer.set_on_segment_start_callback(move |path, _| {
            *callback_path.lock().unwrap() = Some(path.to_path_buf());
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<FlvData, PipelineError>>(8);
        let (payload, _) = OnMetaDataBuilder::new()
            .with_placeholder_keyframes(20)
            .build_bytes(0, false)
            .unwrap();

        tx.blocking_send(Ok(FlvData::Header(FlvHeader::new(false, true))))
            .unwrap();
        tx.blocking_send(Ok(FlvData::Tag(FlvTag::new(
            0,
            0,
            FlvTagType::ScriptData,
            false,
            Bytes::from(payload),
        ))))
        .unwrap();
        tx.blocking_send(Ok(crate::test_utils::create_video_tag(0, true)))
            .unwrap();
        tx.blocking_send(Ok(crate::test_utils::create_video_tag(2_000, true)))
            .unwrap();
        drop(tx);

        writer.run(rx.into()).unwrap();

        let path = opened_path.lock().unwrap().clone().unwrap();
        let file = std::fs::File::open(path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        FlvParser::parse_header(&mut reader).unwrap();
        reader.seek(std::io::SeekFrom::Start(13)).unwrap();
        let (tag, tag_type) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
        assert_eq!(tag_type, FlvTagType::ScriptData);
        let mut cursor = std::io::Cursor::new(tag.data().clone());
        let script = ScriptData::demux(&mut cursor).unwrap();
        let properties = script.data[0].as_object_properties().unwrap();
        let duration = properties
            .iter()
            .find(|(key, _)| key.as_ref() == "duration")
            .map(|(_, value)| value)
            .unwrap();
        assert_eq!(duration, &Amf0Value::Number(2.0));
    }

    #[test]
    fn writer_reserves_and_patches_unreserved_audio_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut writer = RecordingWriter::new(FlvWriterConfig {
            output_dir: tempdir.path().to_path_buf(),
            base_name: "segment-%i".to_string(),
            enable_low_latency: true,
        });
        let opened_path = Arc::new(Mutex::new(None));
        let callback_path = Arc::clone(&opened_path);
        writer.set_on_segment_start_callback(move |path, _| {
            *callback_path.lock().unwrap() = Some(path.to_path_buf());
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<FlvData, PipelineError>>(8);

        tx.blocking_send(Ok(FlvData::Header(FlvHeader::new(true, false))))
            .unwrap();
        tx.blocking_send(Ok(crate::test_utils::create_script_tag(0, false)))
            .unwrap();
        tx.blocking_send(Ok(crate::test_utils::create_audio_tag(0)))
            .unwrap();
        tx.blocking_send(Ok(crate::test_utils::create_audio_tag(2_000)))
            .unwrap();
        drop(tx);

        writer.run(rx.into()).unwrap();

        let path = opened_path.lock().unwrap().clone().unwrap();
        let file = std::fs::File::open(path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        FlvParser::parse_header(&mut reader).unwrap();
        reader.seek(std::io::SeekFrom::Start(13)).unwrap();
        let (tag, tag_type) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
        assert_eq!(tag_type, FlvTagType::ScriptData);
        let mut cursor = std::io::Cursor::new(tag.data().clone());
        let script = ScriptData::demux(&mut cursor).unwrap();
        let properties = script.data[0].as_object_properties().unwrap();
        let duration = properties
            .iter()
            .find(|(key, _)| key.as_ref() == "duration")
            .map(|(_, value)| value)
            .unwrap();
        let has_audio = properties
            .iter()
            .find(|(key, _)| key.as_ref() == "hasAudio")
            .map(|(_, value)| value)
            .unwrap();
        assert_eq!(duration, &Amf0Value::Number(2.0));
        assert_eq!(has_audio, &Amf0Value::Boolean(true));
    }

    #[test]
    fn writer_preserves_filtered_script_payload_even_when_it_is_parseable() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut writer = RecordingWriter::new(FlvWriterConfig {
            output_dir: tempdir.path().to_path_buf(),
            base_name: "segment-%i".to_string(),
            enable_low_latency: true,
        });
        let opened_path = Arc::new(Mutex::new(None));
        let callback_path = Arc::clone(&opened_path);
        writer.set_on_segment_start_callback(move |path, _| {
            *callback_path.lock().unwrap() = Some(path.to_path_buf());
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<FlvData, PipelineError>>(8);
        let FlvData::Tag(mut script_tag) = crate::test_utils::create_script_tag(0, false) else {
            unreachable!();
        };
        script_tag.set_filtered(true);
        let original_payload = script_tag.data().clone();

        tx.blocking_send(Ok(FlvData::Header(FlvHeader::new(true, true))))
            .unwrap();
        tx.blocking_send(Ok(FlvData::Tag(script_tag))).unwrap();
        drop(tx);

        writer.run(rx.into()).unwrap();

        let path = opened_path.lock().unwrap().clone().unwrap();
        let file = std::fs::File::open(path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        FlvParser::parse_header(&mut reader).unwrap();
        reader.seek(std::io::SeekFrom::Start(13)).unwrap();
        let (tag, tag_type) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
        assert_eq!(tag_type, FlvTagType::ScriptData);
        assert!(tag.is_filtered());
        assert_eq!(tag.data(), &original_payload);
    }
}
