//! HLS-specific download orchestrator.
//!
//! This module provides the `HlsDownloader` struct which handles HLS stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.

use futures::StreamExt;
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter};
use mesio::{DownloadStream, MesioDownloaderFactory, ProtocolType};
use parking_lot::RwLock;
use pipeline_common::{PipelineError, PipelineProvider, ProtocolWriter, StreamerContext};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use super::classify_download_error;
use super::config::build_hls_config;
use super::helpers::{self, DownloadStats};
use crate::Result;
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{DownloadConfig, DownloadFailureKind, SegmentEvent};

/// HLS-specific download orchestrator.
///
/// Handles HLS stream downloading with support for both pipeline-processed
/// and raw download modes. Manages stream consumption, writer setup, and
/// event emission through the provided channel.
pub struct HlsDownloader {
    /// Download configuration.
    config: Arc<RwLock<DownloadConfig>>,
    /// Engine-specific configuration.
    engine_config: MesioEngineConfig,
    /// Event sender for segment events.
    event_tx: mpsc::Sender<SegmentEvent>,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Base HLS configuration from the engine.
    hls_config: Option<mesio::hls::HlsConfig>,
}

impl HlsDownloader {
    /// Create a new HLS downloader.
    ///
    /// # Arguments
    /// * `config` - Download configuration with URL, output settings, etc.
    /// * `engine_config` - Mesio engine-specific configuration.
    /// * `event_tx` - Channel sender for emitting segment events.
    /// * `cancellation_token` - Token for graceful cancellation.
    /// * `hls_config` - Optional base HLS configuration from the engine.
    pub fn new(
        config: Arc<RwLock<DownloadConfig>>,
        engine_config: MesioEngineConfig,
        event_tx: mpsc::Sender<SegmentEvent>,
        cancellation_token: CancellationToken,
        hls_config: Option<mesio::hls::HlsConfig>,
    ) -> Self {
        Self {
            config,
            engine_config,
            event_tx,
            cancellation_token,
            hls_config,
        }
    }

    fn config_snapshot(&self) -> DownloadConfig {
        self.config.read().clone()
    }

    /// Create a MesioDownloaderFactory with the configured settings.
    fn create_factory(&self, token: CancellationToken) -> MesioDownloaderFactory {
        let config = self.config_snapshot();
        let hls_config = build_hls_config(&config, self.hls_config.clone(), &self.engine_config);

        MesioDownloaderFactory::new()
            .with_hls_config(hls_config)
            .with_token(token)
    }

    /// Run the HLS download, consuming the stream and writing to files.
    ///
    /// This method:
    /// 1. Creates a MesioDownloaderFactory with the configured HLS settings
    /// 2. Creates a DownloaderInstance::Hls using the factory
    /// 3. Calls download_with_sources() to get the HLS stream
    /// 4. Peeks the first segment to determine file extension (ts vs m4s)
    /// 5. If `enable_processing` is true, routes stream through HlsPipeline
    /// 6. Creates an HlsWriter with callbacks for segment events
    /// 7. Sends HlsData items to the writer via channel
    /// 8. Handles cancellation, progress tracking, and error reporting
    ///
    /// Returns download statistics on success.
    pub async fn run(self) -> Result<DownloadStats> {
        let token = self.cancellation_token.child_token();

        // Create factory with configuration
        let factory = self.create_factory(token.clone());

        let url = self.config_snapshot().url;

        // Create the HLS downloader instance
        let mut downloader = factory
            .create_for_url(&url, ProtocolType::Hls)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create HLS downloader: {}", e)))?;

        // Get the download stream
        let download_stream = downloader
            .download_with_sources(&url)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to start HLS download: {}", e)))?;

        // Extract the HLS stream from the DownloadStream enum
        let mut hls_stream = match download_stream {
            DownloadStream::Hls(stream) => stream,
            _ => {
                return Err(crate::Error::Other(
                    "Expected HLS stream but got different protocol".to_string(),
                ));
            }
        };

        // Peek at the first segment to determine file extension
        let first_segment = loop {
            match hls_stream.next().await {
                Some(Ok(HlsData::EndMarker)) => {
                    debug!("Skipping leading HLS EndMarker before first data segment");
                    continue;
                }
                Some(Ok(segment)) => break segment,
                Some(Err(e)) => {
                    let kind = classify_download_error(&e);
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            kind,
                            message: format!("Failed to get first HLS segment: {}", e),
                        })
                        .await;
                    return Err(crate::Error::Other(format!(
                        "Failed to get first HLS segment: {}",
                        e
                    )));
                }
                None => {
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            kind: DownloadFailureKind::SourceUnavailable,
                            message: "HLS stream is empty".to_string(),
                        })
                        .await;
                    return Err(crate::Error::Other("HLS stream is empty".to_string()));
                }
            }
        };

        // Determine extension from first segment
        let extension = match &first_segment {
            HlsData::TsData(_) => "ts",
            HlsData::M4sData(_) => "m4s",
            HlsData::EndMarker => unreachable!("filtered before extension detection"),
        };

        let config_snapshot = self.config_snapshot();
        info!(
            "Detected HLS stream type for {}: {}, processing_enabled: {}",
            config_snapshot.streamer_id,
            extension.to_uppercase(),
            config_snapshot.enable_processing
        );

        // Route based on enable_processing flag AND engine config
        if config_snapshot.enable_processing && self.engine_config.fix_hls {
            self.download_with_pipeline(token, hls_stream, first_segment, extension)
                .await
        } else {
            self.download_raw(token, hls_stream, first_segment, extension)
                .await
        }
    }

    /// Build HLS writer extras from config.
    fn build_hls_extras(config: &DownloadConfig) -> Option<HashMap<String, String>> {
        if config.max_segment_size_bytes > 0 {
            let mut map = HashMap::new();
            map.insert(
                "max_file_size".to_string(),
                config.max_segment_size_bytes.to_string(),
            );
            Some(map)
        } else {
            None
        }
    }

    /// Download HLS stream with pipeline processing enabled.
    ///
    /// Routes the stream through HlsPipeline for defragmentation, segment splitting,
    /// and other processing before writing to HlsWriter.
    async fn download_with_pipeline(
        &self,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        extension: &str,
    ) -> Result<DownloadStats> {
        let config = self.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting HLS download with pipeline processing for {}",
            streamer_id
        );

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let hls_pipeline_config = config.build_hls_pipeline_config();

        // Create StreamerContext with cancellation token
        let context = StreamerContext::with_name(&streamer_id, token.clone());

        // Create HlsPipeline using PipelineProvider::with_config
        let pipeline_provider =
            HlsPipeline::with_config(context, &pipeline_config, hls_pipeline_config);

        // Build the pipeline (returns ChannelPipeline)
        let pipeline = pipeline_provider.build_pipeline();

        // Spawn the pipeline tasks
        let pipeline_common::channel_pipeline::SpawnedPipeline {
            input_tx: pipeline_input_tx,
            output_rx: pipeline_output_rx,
            tasks: processing_tasks,
        } = pipeline.spawn();

        // Create HlsWriter with callbacks
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();
        let ext = extension.to_string();
        let extras = Self::build_hls_extras(&config);

        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        helpers::setup_writer_callbacks(&mut writer, &self.event_tx);

        // Spawn blocking writer task that reads from pipeline output
        let writer_task = tokio::task::spawn_blocking(move || writer.run(pipeline_output_rx));

        // Send first segment to pipeline input
        if pipeline_input_tx.send(Ok(first_segment)).await.is_err() {
            return Err(crate::Error::Other(
                "Pipeline input channel closed unexpectedly".to_string(),
            ));
        }

        // Consume the rest of the HLS stream and send to pipeline
        let stream_error = helpers::consume_stream(
            hls_stream,
            &pipeline_input_tx,
            &self.cancellation_token,
            &token,
            &streamer_id,
            "HLS",
            classify_download_error,
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
            "HLS",
        )
        .await
    }

    /// Download HLS stream without pipeline processing (raw mode).
    ///
    /// Sends stream data directly to HlsWriter without any processing.
    async fn download_raw(
        &self,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        extension: &str,
    ) -> Result<DownloadStats> {
        let config = self.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting HLS download without pipeline processing for {}",
            streamer_id
        );

        // Build pipeline config for channel size
        let pipeline_config = config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<HlsData, PipelineError>>(channel_size);

        // Create HlsWriter with callbacks
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();
        let ext = extension.to_string();
        let extras = Self::build_hls_extras(&config);

        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        helpers::setup_writer_callbacks(&mut writer, &self.event_tx);

        // Spawn blocking writer task
        let writer_task = tokio::task::spawn_blocking(move || writer.run(rx));

        // Send first segment to writer
        if tx.send(Ok(first_segment)).await.is_err() {
            return Err(crate::Error::Other(
                "Writer channel closed unexpectedly".to_string(),
            ));
        }

        // Consume the rest of the HLS stream and send to writer
        let stream_error = helpers::consume_stream(
            hls_stream,
            &tx,
            &self.cancellation_token,
            &token,
            &streamer_id,
            "HLS",
            classify_download_error,
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
            "HLS",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn download_raw_emits_segment_completed_before_download_failed_on_stream_error() {
        let temp = tempfile::tempdir().expect("tempdir");

        let config = DownloadConfig::new(
            "http://example.invalid/stream.m3u8",
            temp.path().to_path_buf(),
            "streamer",
            "session",
        )
        .with_filename_template("test-hls");

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<SegmentEvent>(32);
        let downloader = HlsDownloader::new(
            Arc::new(RwLock::new(config)),
            MesioEngineConfig::default(),
            event_tx,
            CancellationToken::new(),
            None,
        );

        let first_segment = HlsData::ts(
            MediaSegment {
                uri: "seg0.ts".to_string(),
                duration: 1.0,
                ..Default::default()
            },
            Bytes::from_static(&[0_u8; 188]),
        );

        let hls_stream = futures::stream::iter([
            Ok(HlsData::ts(
                MediaSegment {
                    uri: "seg1.ts".to_string(),
                    duration: 1.0,
                    ..Default::default()
                },
                Bytes::from_static(&[1_u8; 188]),
            )),
            Err(mesio::hls::HlsDownloaderError::Playlist {
                reason: "simulated stream error".to_string(),
            }),
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
            .download_raw(CancellationToken::new(), hls_stream, first_segment, "ts")
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
