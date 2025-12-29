//! FLV-specific download orchestrator.
//!
//! This module provides the `FlvDownloader` struct which handles FLV stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.

use chrono::Utc;
use flv::data::FlvData;
use flv_fix::{FlvPipeline, FlvWriter};
use futures::StreamExt;
use mesio::flv::FlvProtocolConfig;
use mesio::flv::error::FlvDownloadError;
use mesio::{DownloadStream, MesioDownloaderFactory, ProtocolType};
use parking_lot::RwLock;
use pipeline_common::{PipelineError, PipelineProvider, ProtocolWriter, StreamerContext};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::config::build_flv_config;
use super::hls_downloader::DownloadStats;
use crate::Result;
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{
    DownloadConfig, DownloadProgress, SegmentEvent, SegmentInfo,
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
    pub async fn run(self) -> Result<DownloadStats> {
        let token = CancellationToken::new();

        // Create factory with configuration
        let factory = self.create_factory(token.clone());

        let url = self.config_snapshot().url;

        // Create the FLV downloader instance
        let mut downloader = factory
            .create_for_url(&url, ProtocolType::Flv)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create FLV downloader: {}", e)))?;

        // Add the source URL
        downloader.add_source(&url, 0);

        // Get the download stream
        let download_stream = downloader
            .download_with_sources(&url)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to start FLV download: {}", e)))?;

        // Extract the FLV stream from the DownloadStream enum
        let flv_stream = match download_stream {
            DownloadStream::Flv(stream) => stream,
            _ => {
                return Err(crate::Error::Other(
                    "Expected FLV stream but got different protocol".to_string(),
                ));
            }
        };

        let config_snapshot = self.config_snapshot();
        info!(
            "Starting FLV download for {}, processing_enabled: {}",
            config_snapshot.streamer_id, config_snapshot.enable_processing
        );

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
    ) -> Result<DownloadStats> {
        let config = self.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting FLV download with pipeline processing for {}",
            streamer_id
        );

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let flv_pipeline_config = config.build_flv_pipeline_config();

        // Create StreamerContext with cancellation token
        let context = StreamerContext::new(token.clone());

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

        // Build extras for enable_low_latency
        let extras = {
            let mut map = HashMap::new();
            map.insert("enable_low_latency".to_string(), "true".to_string());
            Some(map)
        };

        let mut writer = FlvWriter::new(output_dir, base_name, "flv".to_string(), extras);

        // SegmentStarted/SegmentCompleted must not be dropped (danmu segmentation + pipelines rely on it).
        // These callbacks run on a blocking thread; use `blocking_send` to apply backpressure
        // rather than unbounded buffering.
        let event_tx_start = self.event_tx.clone();
        let event_tx_complete = self.event_tx.clone();

        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx_start.blocking_send(event);
        });

        writer.set_on_segment_complete_callback(
            move |path, sequence, duration_secs, size_bytes| {
                // Ensure path is absolute
                let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                    path: abs_path,
                    duration_secs,
                    size_bytes,
                    index: sequence,
                    completed_at: Utc::now(),
                });
                let _ = event_tx_complete.blocking_send(event);
            },
        );

        // Setup progress callback
        let event_tx_progress = self.event_tx.clone();
        writer.set_progress_callback(move |progress| {
            let download_progress = DownloadProgress {
                bytes_downloaded: progress.bytes_written_total,
                duration_secs: progress.elapsed_secs,
                speed_bytes_per_sec: progress.speed_bytes_per_sec,
                segments_completed: progress.current_file_sequence,
                current_segment: None,
                media_duration_secs: progress.media_duration_secs_total,
                playback_ratio: progress.playback_ratio,
            };
            let _ = event_tx_progress.try_send(SegmentEvent::Progress(download_progress));
        });

        // Spawn blocking writer task that reads from pipeline output
        let writer_task = tokio::task::spawn_blocking(move || writer.run(pipeline_output_rx));

        // Consume the FLV stream and send to pipeline
        let mut stream = std::pin::pin!(flv_stream);
        let mut stream_error: Option<String> = None;

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if self.cancellation_token.is_cancelled() || token.is_cancelled() {
                debug!("FLV download cancelled for {}", streamer_id);
                break;
            }

            match result {
                Ok(flv_data) => {
                    // Send to pipeline input
                    if pipeline_input_tx.send(Ok(flv_data)).await.is_err() {
                        warn!("Pipeline input channel closed, stopping FLV download");
                        break;
                    }
                }
                Err(e) => {
                    error!("FLV stream error for {}: {}", streamer_id, e);
                    let err = e.to_string();
                    stream_error = Some(err.clone());
                    let _ = pipeline_input_tx
                        .send(Err(PipelineError::Processing(err)))
                        .await;
                    break;
                }
            }
        }

        // Close the pipeline input channel to signal completion
        drop(pipeline_input_tx);

        // Wait for writer to complete
        let writer_result = writer_task
            .await
            .map_err(|e| crate::Error::Other(format!("Writer task panicked: {}", e)))?;

        // Wait for processing tasks to complete
        for task in processing_tasks {
            let task_result = task
                .await
                .map_err(|e| crate::Error::Other(format!("Pipeline task panicked: {}", e)))?;

            // Only report task errors if writer succeeded
            if writer_result.is_ok()
                && let Err(e) = task_result
            {
                warn!("Pipeline processing task error: {}", e);
            }
        }

        match writer_result {
            Ok((items_written, files_created, total_bytes, total_duration)) => {
                // Get final stats from writer state
                let stats = DownloadStats {
                    total_bytes,
                    total_items: items_written,
                    total_duration_secs: total_duration,
                    files_created: files_created + 1,
                };

                if let Some(err) = &stream_error {
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: err.clone(),
                            recoverable: true,
                        })
                        .await;
                    return Err(crate::Error::Other(format!("FLV stream error: {}", err)));
                }

                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: stats.total_bytes,
                        total_duration_secs: stats.total_duration_secs,
                        total_segments: stats.files_created,
                    })
                    .await;

                info!(
                    "FLV download with pipeline completed for {}: {} items, {} files",
                    streamer_id, items_written, stats.files_created
                );

                Ok(stats)
            }
            Err(e) => {
                if let Some(err) = stream_error {
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: err.clone(),
                            recoverable: true,
                        })
                        .await;
                    return Err(crate::Error::Other(format!("FLV stream error: {}", err)));
                }
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: e.to_string(),
                        recoverable: false,
                    })
                    .await;
                Err(crate::Error::Other(format!("FLV writer error: {}", e)))
            }
        }
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
    ) -> Result<DownloadStats> {
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

        // Build extras for enable_low_latency
        let extras = {
            let mut map = HashMap::new();
            map.insert("enable_low_latency".to_string(), "true".to_string());
            Some(map)
        };

        let mut writer = FlvWriter::new(output_dir, base_name, "flv".to_string(), extras);

        // SegmentStarted/SegmentCompleted must not be dropped (danmu segmentation + pipelines rely on it).
        // These callbacks run on a blocking thread; use `blocking_send` to apply backpressure
        // rather than unbounded buffering.
        let event_tx_start = self.event_tx.clone();
        let event_tx_complete = self.event_tx.clone();

        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx_start.blocking_send(event);
        });

        writer.set_on_segment_complete_callback(
            move |path, sequence, duration_secs, size_bytes| {
                // Ensure path is absolute
                let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                    path: abs_path,
                    duration_secs,
                    size_bytes,
                    index: sequence,
                    completed_at: Utc::now(),
                });
                let _ = event_tx_complete.blocking_send(event);
            },
        );

        // Setup progress callback
        let event_tx_progress = self.event_tx.clone();
        writer.set_progress_callback(move |progress| {
            let download_progress = DownloadProgress {
                bytes_downloaded: progress.bytes_written_total,
                duration_secs: progress.elapsed_secs,
                speed_bytes_per_sec: progress.speed_bytes_per_sec,
                segments_completed: progress.current_file_sequence,
                current_segment: None,
                media_duration_secs: progress.media_duration_secs_total,
                playback_ratio: progress.playback_ratio,
            };
            let _ = event_tx_progress.try_send(SegmentEvent::Progress(download_progress));
        });

        // Spawn blocking writer task
        let writer_task = tokio::task::spawn_blocking(move || writer.run(rx));

        // Consume the FLV stream and send to writer
        let mut stream = std::pin::pin!(flv_stream);
        let mut stream_error: Option<String> = None;

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if self.cancellation_token.is_cancelled() || token.is_cancelled() {
                debug!("FLV download cancelled for {}", streamer_id);
                break;
            }

            match result {
                Ok(flv_data) => {
                    // Send to writer
                    if tx.send(Ok(flv_data)).await.is_err() {
                        warn!("Writer channel closed, stopping FLV download");
                        break;
                    }
                }
                Err(e) => {
                    // Stream error - send error to writer and emit failure event
                    error!("FLV stream error for {}: {}", streamer_id, e);
                    let err = e.to_string();
                    stream_error = Some(err.clone());
                    let _ = tx.send(Err(PipelineError::Processing(err))).await;
                    break;
                }
            }
        }

        // Close the channel to signal writer to finish
        drop(tx);

        // Wait for writer to complete and get final stats
        let writer_result = writer_task
            .await
            .map_err(|e| crate::Error::Other(format!("Writer task panicked: {}", e)))?;

        match writer_result {
            Ok((items_written, files_created, total_bytes, total_duration)) => {
                // Get final stats from writer state
                let stats = DownloadStats {
                    total_bytes,
                    total_items: items_written,
                    total_duration_secs: total_duration,
                    files_created: files_created + 1,
                };

                if let Some(err) = &stream_error {
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: err.clone(),
                            recoverable: true,
                        })
                        .await;
                    return Err(crate::Error::Other(format!("FLV stream error: {}", err)));
                }

                // Emit completion event with stats from writer
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: stats.total_bytes,
                        total_duration_secs: stats.total_duration_secs,
                        total_segments: stats.files_created,
                    })
                    .await;

                info!(
                    "FLV download completed for {}: {} items, {} files",
                    streamer_id, items_written, stats.files_created
                );

                Ok(stats)
            }
            Err(e) => {
                if let Some(err) = stream_error {
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: err.clone(),
                            recoverable: true,
                        })
                        .await;
                    return Err(crate::Error::Other(format!("FLV stream error: {}", err)));
                }
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: e.to_string(),
                        recoverable: false,
                    })
                    .await;
                Err(crate::Error::Other(format!("FLV writer error: {}", e)))
            }
        }
    }
}
