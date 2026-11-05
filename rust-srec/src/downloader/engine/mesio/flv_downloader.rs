//! FLV-specific download orchestrator.
//!
//! This module provides the `FlvDownloader` struct which handles FLV stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.

use flv::data::FlvData;
use flv_fix::{FlvPipeline, FlvPipelineConfig, FlvWriter, FlvWriterConfig};
use mesio::{DownloadError, DownloadRequest, MesioConfig, MesioDownloader, ProtocolSelection};
use pipeline_common::{
    ChannelSpec, PipelineError, PipelineProvider, ProtocolWriter, StreamerContext, spawn_pipeline,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::config::build_flv_config;
use super::helpers::{self, DownloadStats};
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{DownloadHandle, EngineStartError};

/// FLV-specific download orchestrator.
///
/// Handles FLV stream downloading with support for both pipeline-processed
/// and raw download modes. Manages stream consumption, writer setup, and
/// event emission through the provided channel.
pub(super) struct FlvDownloader {
    /// The download attempt: shared configuration, event sender, cancellation
    /// token and manual-split control.
    handle: Arc<DownloadHandle>,
    /// Engine-specific configuration.
    engine_config: MesioEngineConfig,
}

impl FlvDownloader {
    /// Create a new FLV downloader.
    ///
    /// # Arguments
    /// * `handle` - The download attempt to record.
    /// * `engine_config` - Mesio engine-specific configuration.
    pub(super) fn new(handle: Arc<DownloadHandle>, engine_config: MesioEngineConfig) -> Self {
        Self {
            handle,
            engine_config,
        }
    }

    /// Create a MesioDownloader with the configured settings.
    fn create_downloader(&self, token: CancellationToken) -> MesioDownloader {
        let config = self.handle.config_snapshot();
        let flv_config = build_flv_config(&config, &self.engine_config);

        MesioDownloader::new(MesioConfig {
            hls: mesio::hls::HlsConfig::default(),
            flv: flv_config,
            dash: Default::default(),
            token,
        })
    }

    /// Run the FLV download, consuming the stream and writing to files.
    ///
    /// This method:
    /// 1. Creates a MesioDownloader with the configured FLV settings
    /// 2. Starts a typed FLV download session
    /// 4. If `enable_processing` is true, routes stream through FlvPipeline
    /// 5. Creates a FlvWriter with callbacks for segment events
    /// 6. Sends FlvData items to the writer via channel
    /// 7. Handles cancellation, progress tracking, and error reporting
    ///
    /// Returns download statistics on success.
    pub(super) async fn run(self) -> std::result::Result<DownloadStats, EngineStartError> {
        let token = self.handle.cancellation_token.child_token();

        let downloader = self.create_downloader(token.clone());

        let url = self.handle.config_snapshot().url;

        let request = DownloadRequest::from_url(&url)
            .map_err(|e| {
                let kind = super::classify_download_error(&e);
                EngineStartError::new(kind, format!("Invalid FLV download URL: {}", e))
            })?
            .with_protocol(ProtocolSelection::Flv(Default::default()))
            .with_cancel(token.clone());

        let session = downloader.start_flv(request).await.map_err(|e| {
            let kind = super::classify_download_error(&e);
            EngineStartError::new(kind, format!("Failed to start FLV download: {}", e))
        })?;
        self.download(token, session.items).await
    }

    /// Writes the FLV stream to files, through the FLV pipeline
    /// (defragmentation, GOP sorting, timing repair and other processing)
    /// when processing is enabled for the download and FLV, or straight to
    /// the writer otherwise.
    async fn download(
        &self,
        token: CancellationToken,
        flv_stream: impl futures::Stream<Item = std::result::Result<FlvData, DownloadError>>
        + Send
        + Unpin,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let config = self.handle.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        let processing = config.enable_processing && self.engine_config.fix_flv;
        info!(streamer_id = %streamer_id, processing, "Starting FLV download");

        let pipeline_config = config.build_pipeline_config();
        let (tx, writer_rx, processing_tasks) = if processing {
            let mut flv_pipeline_config = FlvPipelineConfig::default();
            if let Some(ref opts) = self.engine_config.flv_fix {
                opts.apply_to(&mut flv_pipeline_config);
            }

            // Manual split needs the pipeline, so only this mode enables it.
            let context = Arc::new(
                StreamerContext::with_name(&streamer_id, token.clone())
                    .with_manual_split(self.handle.manual_split.clone()),
            );
            self.handle.manual_split.enable();

            let pipeline = FlvPipeline::with_config(context, &pipeline_config, flv_pipeline_config)
                .build_pipeline();
            let pipeline_common::SpawnedPipeline {
                input_tx,
                output_rx,
                tasks,
            } = spawn_pipeline(pipeline, ChannelSpec::items(pipeline_config.channel_size));
            (helpers::WriterInput::Pipeline(input_tx), output_rx, tasks)
        } else {
            let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<FlvData, PipelineError>>(
                pipeline_config.channel_size,
            );
            (helpers::WriterInput::Raw(tx), rx.into(), Vec::new())
        };

        let mut writer = FlvWriter::new(FlvWriterConfig {
            output_dir: config.output_dir.clone(),
            base_name: config.filename_template.clone(),
        });

        helpers::setup_writer_callbacks(&mut writer, &self.handle.event_tx);

        let writer_task = tokio::task::spawn_blocking(move || writer.run(writer_rx));

        let stream_error = helpers::consume_stream(
            flv_stream,
            &tx,
            helpers::StreamConsumeContext {
                managed_credentials: config.managed_credentials,
                parent_token: &self.handle.cancellation_token,
                child_token: &token,
                streamer_id: &streamer_id,
                protocol: "FLV",
            },
            super::classify_download_error,
            |_| {},
        )
        .await;

        // Closing the input lets the writer finish.
        drop(tx);

        helpers::handle_writer_result(
            writer_task,
            stream_error,
            processing_tasks,
            helpers::WriterSettleContext {
                managed_credentials: config.managed_credentials,
                event_tx: &self.handle.event_tx,
                streamer_id: &streamer_id,
                protocol: "FLV",
                output_dir: &config.output_dir,
                // FLV streams end on a TCP close. That's "we got disconnected"
                // — could be EOF, could be the streamer's network blipping.
                // SessionLifecycle treats this as ambiguous and enters
                // hysteresis to absorb a possible reconnect.
                engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
            },
        )
        .await
        .map_err(EngineStartError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::engine::traits::{DownloadConfig, EngineType, SegmentEvent};
    use bytes::Bytes;
    use flv::header::FlvHeader;
    use flv::tag::{FlvTag, FlvTagType};
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn download_raw_emits_segment_completed_before_download_failed_on_stream_error() {
        let temp = tempfile::tempdir().expect("tempdir");

        let config = DownloadConfig::new(
            "http://example.invalid/stream.flv",
            temp.path().to_path_buf(),
            "streamer",
            "streamer",
            "session",
        )
        .with_filename_template("test-flv")
        .with_processing_enabled(false);

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<SegmentEvent>(32);
        let handle = DownloadHandle::new("download", EngineType::Mesio, config, event_tx);
        let downloader = FlvDownloader::new(Arc::new(handle), MesioEngineConfig::default());

        let header = FlvData::Header(FlvHeader::new(true, true));
        let tag = FlvData::Tag(FlvTag::new(
            0,
            0,
            FlvTagType::ScriptData,
            false,
            Bytes::new(),
        ));

        let flv_stream = futures::stream::iter([
            Ok(header),
            Ok(tag),
            Err(DownloadError::source_exhausted("simulated stream error")),
        ]);

        let events_task = tokio::spawn(async move {
            let mut events = Vec::new();
            loop {
                let next = timeout(Duration::from_secs(5), event_rx.recv())
                    .await
                    .expect("event recv timeout");
                let Some(ev) = next else {
                    break;
                };
                events.push(ev.clone());
                if matches!(ev, SegmentEvent::DownloadFailed { .. }) {
                    break;
                }
            }
            events
        });

        let result = downloader
            .download(CancellationToken::new(), flv_stream)
            .await;
        assert!(result.is_err(), "expected stream error");

        let events = events_task.await.expect("events task join");

        let completed_idx = events
            .iter()
            .position(|e| matches!(e, SegmentEvent::SegmentCompleted(_)))
            .expect("expected SegmentCompleted");
        let failed_idx = events
            .iter()
            .position(|e| matches!(e, SegmentEvent::DownloadFailed { .. }))
            .expect("expected DownloadFailed");

        assert!(
            completed_idx < failed_idx,
            "expected SegmentCompleted before DownloadFailed, got: {:?}",
            events
                .iter()
                .map(|e| match e {
                    SegmentEvent::SegmentStarted { .. } => "SegmentStarted",
                    SegmentEvent::SegmentCompleted(_) => "SegmentCompleted",
                    SegmentEvent::Progress(_) => "Progress",
                    SegmentEvent::DownloadCompleted { .. } => "DownloadCompleted",
                    SegmentEvent::DownloadFailed { .. } => "DownloadFailed",
                    SegmentEvent::OutputIoError { .. } => "OutputIoError",
                })
                .collect::<Vec<_>>()
        );
    }
}
