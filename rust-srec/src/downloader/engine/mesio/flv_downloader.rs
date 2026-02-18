//! FLV-specific download orchestrator.
//!
//! This module provides the `FlvDownloader` struct which handles FLV stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.

use flv::data::FlvData;
use flv_fix::{FlvPipeline, FlvPipelineConfig, FlvWriter, FlvWriterConfig};
use mesio::flv::FlvProtocolConfig;
use mesio::flv::error::FlvDownloadError;
use mesio::{DownloadStream, MesioDownloaderFactory, ProtocolType};
use parking_lot::RwLock;
use pipeline_common::{PipelineError, PipelineProvider, ProtocolWriter, StreamerContext};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::classify_flv_error;
use super::config::build_flv_config;
use super::helpers::{self, DownloadStats};
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{
    DownloadConfig, DownloadFailureKind, EngineStartError, SegmentEvent,
};

/// FLV-specific download orchestrator.
///
/// Handles FLV stream downloading with support for both pipeline-processed
/// and raw download modes. Manages stream consumption, writer setup, and
/// event emission through the provided channel.
pub struct FlvDownloader {
    /// Download configuration.
    config: Arc<RwLock<DownloadConfig>>,
    /// Engine-specific configuration.
    engine_config: MesioEngineConfig,
    /// Event sender for segment events.
    event_tx: mpsc::Sender<SegmentEvent>,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Base FLV configuration from the engine.
    flv_config: Option<FlvProtocolConfig>,
}

impl FlvDownloader {
    /// Create a new FLV downloader.
    ///
    /// # Arguments
    /// * `config` - Download configuration with URL, output settings, etc.
    /// * `engine_config` - Mesio engine-specific configuration.
    /// * `event_tx` - Channel sender for emitting segment events.
    /// * `cancellation_token` - Token for graceful cancellation.
    /// * `flv_config` - Optional base FLV configuration from the engine.
    pub fn new(
        config: Arc<RwLock<DownloadConfig>>,
        engine_config: MesioEngineConfig,
        event_tx: mpsc::Sender<SegmentEvent>,
        cancellation_token: CancellationToken,
        flv_config: Option<FlvProtocolConfig>,
    ) -> Self {
        Self {
            config,
            engine_config,
            event_tx,
            cancellation_token,
            flv_config,
        }
    }

    fn config_snapshot(&self) -> DownloadConfig {
        self.config.read().clone()
    }

    /// Create a MesioDownloaderFactory with the configured settings.
    fn create_factory(&self, token: CancellationToken) -> MesioDownloaderFactory {
        let config = self.config_snapshot();
        let flv_config = build_flv_config(&config, self.flv_config.clone());

        MesioDownloaderFactory::new()
            .with_flv_config(flv_config)
            .with_token(token)
    }

    /// Run the FLV download, consuming the stream and writing to files.
    ///
    /// This method:
    /// 1. Creates a MesioDownloaderFactory with the configured FLV settings
    /// 2. Creates a DownloaderInstance::Flv using the factory
    /// 3. Calls download_with_sources() to get the FLV stream
    /// 4. If `enable_processing` is true, routes stream through FlvPipeline
    /// 5. Creates a FlvWriter with callbacks for segment events
    /// 6. Sends FlvData items to the writer via channel
    /// 7. Handles cancellation, progress tracking, and error reporting
    ///
    /// Returns download statistics on success.
    pub async fn run(self) -> std::result::Result<DownloadStats, EngineStartError> {
        let token = self.cancellation_token.child_token();

        // Create factory with configuration
        let factory = self.create_factory(token.clone());

        let url = self.config_snapshot().url;

        // Create the FLV downloader instance
        let mut downloader = factory
            .create_for_url(&url, ProtocolType::Flv)
            .await
            .map_err(|e| {
                let kind = super::classify_download_error(&e);
                EngineStartError::new(kind, format!("Failed to create FLV downloader: {}", e))
            })?;

        // Add the source URL
        downloader.add_source(&url, 0);

        // Get the download stream
        let download_stream = downloader.download_with_sources(&url).await.map_err(|e| {
            let kind = super::classify_download_error(&e);
            EngineStartError::new(kind, format!("Failed to start FLV download: {}", e))
        })?;

        // Extract the FLV stream from the DownloadStream enum
        let flv_stream = match download_stream {
            DownloadStream::Flv(stream) => stream,
            _ => {
                return Err(EngineStartError::new(
                    DownloadFailureKind::Configuration,
                    "Expected FLV stream but got different protocol",
                ));
            }
        };

        let config_snapshot = self.config_snapshot();

        // Route based on enable_processing flag AND engine config
        if config_snapshot.enable_processing && self.engine_config.fix_flv {
            self.download_with_pipeline(token, flv_stream).await
        } else {
            self.download_raw(token, flv_stream).await
        }
    }

    /// Download FLV stream with pipeline processing enabled.
    ///
    /// Routes the stream through FlvPipeline for defragmentation, GOP sorting,
    /// timing repair, and other processing before writing to FlvWriter.
    async fn download_with_pipeline(
        &self,
        token: CancellationToken,
        flv_stream: impl futures::Stream<Item = std::result::Result<FlvData, FlvDownloadError>>
        + Send
        + Unpin,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let config = self.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(streamer_id = %streamer_id, "Starting FLV download with pipeline processing");

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let flv_pipeline_config = if let Some(cfg) = config.flv_pipeline_config.clone() {
            cfg
        } else {
            let mut cfg = FlvPipelineConfig::default();
            if let Some(ref opts) = self.engine_config.flv_fix {
                opts.apply_to(&mut cfg);
            }
            cfg
        };

        // Create StreamerContext with streamer name and cancellation token
        let context = Arc::new(StreamerContext::with_name(&streamer_id, token.clone()));

        // Create FlvPipeline using PipelineProvider::with_config
        let pipeline_provider =
            FlvPipeline::with_config(context, &pipeline_config, flv_pipeline_config);

        // Build the pipeline (returns ChannelPipeline)
        let pipeline = pipeline_provider.build_pipeline();

        // Spawn the pipeline tasks
        let pipeline_common::channel_pipeline::SpawnedPipeline {
            input_tx: pipeline_input_tx,
            output_rx: pipeline_output_rx,
            tasks: processing_tasks,
        } = pipeline.spawn();

        // Create FlvWriter with callbacks
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();

        let mut writer = FlvWriter::new(FlvWriterConfig {
            output_dir,
            base_name,
            enable_low_latency: true,
        });

        helpers::setup_writer_callbacks(&mut writer, &self.event_tx);

        // Spawn blocking writer task that reads from pipeline output
        let writer_task = tokio::task::spawn_blocking(move || writer.run(pipeline_output_rx));

        // Consume the FLV stream and send to pipeline
        let stream_error = helpers::consume_stream(
            flv_stream,
            &pipeline_input_tx,
            &self.cancellation_token,
            &token,
            &streamer_id,
            "FLV",
            classify_flv_error,
        )
        .await;

        // Close the pipeline input channel to signal completion
        drop(pipeline_input_tx);

        // Wait for writer to complete
        let writer_result = writer_task
            .await
            .map_err(|e| crate::Error::Other(format!("Writer task panicked: {}", e)))?;

        helpers::handle_writer_result(
            writer_result,
            stream_error,
            processing_tasks,
            &self.event_tx,
            &streamer_id,
            "FLV",
        )
        .await
        .map_err(EngineStartError::from)
    }

    /// Download FLV stream without pipeline processing (raw mode).
    ///
    /// Sends stream data directly to FlvWriter without any processing.
    async fn download_raw(
        &self,
        token: CancellationToken,
        flv_stream: impl futures::Stream<Item = std::result::Result<FlvData, FlvDownloadError>>
        + Send
        + Unpin,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let config = self.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting FLV download without pipeline processing for {}",
            streamer_id
        );

        // Build pipeline config for channel size
        let pipeline_config = config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<FlvData, PipelineError>>(channel_size);

        // Create FlvWriter with callbacks
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();

        let mut writer = FlvWriter::new(FlvWriterConfig {
            output_dir,
            base_name,
            enable_low_latency: true,
        });

        helpers::setup_writer_callbacks(&mut writer, &self.event_tx);

        // Spawn blocking writer task
        let writer_task = tokio::task::spawn_blocking(move || writer.run(rx));

        // Consume the FLV stream and send to writer
        let stream_error = helpers::consume_stream(
            flv_stream,
            &tx,
            &self.cancellation_token,
            &token,
            &streamer_id,
            "FLV",
            classify_flv_error,
        )
        .await;

        // Close the channel to signal writer to finish
        drop(tx);

        // Wait for writer to complete and get final stats
        let writer_result = writer_task
            .await
            .map_err(|e| crate::Error::Other(format!("Writer task panicked: {}", e)))?;

        helpers::handle_writer_result(
            writer_result,
            stream_error,
            vec![],
            &self.event_tx,
            &streamer_id,
            "FLV",
        )
        .await
        .map_err(EngineStartError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        .with_filename_template("test-flv");

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<SegmentEvent>(32);
        let downloader = FlvDownloader::new(
            Arc::new(RwLock::new(config)),
            MesioEngineConfig::default(),
            event_tx,
            CancellationToken::new(),
            None,
        );

        let header = FlvData::Header(FlvHeader::new(true, true));
        let tag = FlvData::Tag(FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            is_filtered: false,
            data: Bytes::new(),
        });

        let flv_stream = futures::stream::iter([
            Ok(header),
            Ok(tag),
            Err(FlvDownloadError::AllSourcesFailed(
                "simulated stream error".to_string(),
            )),
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
            .download_raw(CancellationToken::new(), flv_stream)
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
                })
                .collect::<Vec<_>>()
        );
    }
}
