//! HLS-specific download orchestrator.
//!
//! This module provides the `HlsDownloader` struct which handles HLS stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.

use chrono::Utc;
use futures::StreamExt;
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter};
use mesio::{DownloadStream, MesioDownloaderFactory, ProtocolType};
use pipeline_common::{PipelineError, PipelineProvider, ProtocolWriter, StreamerContext};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::config::build_hls_config;
use crate::downloader::engine::traits::{DownloadConfig, DownloadProgress, SegmentEvent, SegmentInfo};
use crate::database::models::engine::MesioEngineConfig;
use crate::Result;

/// Statistics returned after download completes.
#[derive(Debug, Clone, Default)]
pub struct DownloadStats {
    /// Total bytes written across all files.
    pub total_bytes: u64,
    /// Total items (segments/tags) written.
    pub total_items: usize,
    /// Total media duration in seconds.
    pub total_duration_secs: f64,
    /// Number of files created.
    pub files_created: u32,
}

/// HLS-specific download orchestrator.
///
/// Handles HLS stream downloading with support for both pipeline-processed
/// and raw download modes. Manages stream consumption, writer setup, and
/// event emission through the provided channel.
pub struct HlsDownloader {
    /// Download configuration.
    config: DownloadConfig,
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
        config: DownloadConfig,
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

    /// Create a MesioDownloaderFactory with the configured settings.
    fn create_factory(&self, token: CancellationToken) -> MesioDownloaderFactory {
        let hls_config = build_hls_config(&self.config, self.hls_config.clone());

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
        let token = CancellationToken::new();

        // Create factory with configuration
        let factory = self.create_factory(token.clone());

        // Create the HLS downloader instance
        let mut downloader = factory
            .create_for_url(&self.config.url, ProtocolType::Hls)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create HLS downloader: {}", e)))?;

        // Get the download stream
        let download_stream = downloader
            .download_with_sources(&self.config.url)
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
        let first_segment = match hls_stream.next().await {
            Some(Ok(segment)) => segment,
            Some(Err(e)) => {
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: format!("Failed to get first HLS segment: {}", e),
                        recoverable: true,
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
                        error: "HLS stream is empty".to_string(),
                        recoverable: false,
                    })
                    .await;
                return Err(crate::Error::Other("HLS stream is empty".to_string()));
            }
        };

        // Determine extension from first segment
        let extension = match &first_segment {
            HlsData::TsData(_) => "ts",
            HlsData::M4sData(_) => "m4s",
            HlsData::EndMarker => {
                return Err(crate::Error::Other(
                    "First segment is EndMarker".to_string(),
                ));
            }
        };

        info!(
            "Detected HLS stream type for {}: {}, processing_enabled: {}",
            self.config.streamer_id,
            extension.to_uppercase(),
            self.config.enable_processing
        );

        // Route based on enable_processing flag AND engine config
        if self.config.enable_processing && self.engine_config.fix_hls {
            self.download_with_pipeline(token, hls_stream, first_segment, extension)
                .await
        } else {
            self.download_raw(token, hls_stream, first_segment, extension)
                .await
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
        info!(
            "Starting HLS download with pipeline processing for {}",
            self.config.streamer_id
        );

        // Build pipeline and common configs
        let pipeline_config = self.config.build_pipeline_config();
        let hls_pipeline_config = self.config.build_hls_pipeline_config();

        // Create StreamerContext with cancellation token
        let context = StreamerContext::new(token.clone());

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
        let output_dir = self.config.output_dir.clone();
        let base_name = self.config.filename_template.clone();
        let ext = extension.to_string();

        // Build extras for max_file_size if configured
        let extras = if self.config.max_segment_size_bytes > 0 {
            let mut map = HashMap::new();
            map.insert(
                "max_file_size".to_string(),
                self.config.max_segment_size_bytes.to_string(),
            );
            Some(map)
        } else {
            None
        };

        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        // Setup callbacks for segment events
        let event_tx_start = self.event_tx.clone();
        let event_tx_complete = self.event_tx.clone();

        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx_start.try_send(event);
        });

        writer.set_on_segment_complete_callback(move |path, sequence, duration_secs| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_complete.try_send(event);
        });

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

        // Send first segment to pipeline input
        if pipeline_input_tx.send(Ok(first_segment)).await.is_err() {
            return Err(crate::Error::Other(
                "Pipeline input channel closed unexpectedly".to_string(),
            ));
        }

        // Consume the rest of the HLS stream and send to pipeline
        let mut stream = std::pin::pin!(hls_stream);

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if self.cancellation_token.is_cancelled() || token.is_cancelled() {
                debug!(
                    "HLS download cancelled for {}",
                    self.config.streamer_id
                );
                break;
            }

            match result {
                Ok(hls_data) => {
                    // Send to pipeline input
                    if pipeline_input_tx.send(Ok(hls_data)).await.is_err() {
                        warn!("Pipeline input channel closed, stopping HLS download");
                        break;
                    }
                }
                Err(e) => {
                    error!(
                        "HLS stream error for {}: {}",
                        self.config.streamer_id, e
                    );
                    let _ = pipeline_input_tx
                        .send(Err(PipelineError::Processing(e.to_string())))
                        .await;
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: e.to_string(),
                            recoverable: true,
                        })
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
            if writer_result.is_ok() {
                if let Err(e) = task_result {
                    warn!("Pipeline processing task error: {}", e);
                }
            }
        }

        match writer_result {
            Ok((items_written, files_created)) => {
                // Get final stats from writer state
                let stats = DownloadStats {
                    total_bytes: 0, // Will be updated from writer state when available
                    total_items: items_written,
                    total_duration_secs: 0.0, // Will be updated from writer state when available
                    files_created: files_created + 1,
                };

                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: stats.total_bytes,
                        total_duration_secs: stats.total_duration_secs,
                        total_segments: stats.files_created,
                    })
                    .await;

                info!(
                    "HLS download with pipeline completed for {}: {} items, {} files",
                    self.config.streamer_id, items_written, stats.files_created
                );

                Ok(stats)
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: e.to_string(),
                        recoverable: false,
                    })
                    .await;
                Err(crate::Error::Other(format!("HLS writer error: {}", e)))
            }
        }
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
        info!(
            "Starting HLS download without pipeline processing for {}",
            self.config.streamer_id
        );

        // Build pipeline config for channel size
        let pipeline_config = self.config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<HlsData, PipelineError>>(channel_size);

        // Create HlsWriter with callbacks
        let output_dir = self.config.output_dir.clone();
        let base_name = self.config.filename_template.clone();
        let ext = extension.to_string();

        // Build extras for max_file_size if configured
        let extras = if self.config.max_segment_size_bytes > 0 {
            let mut map = HashMap::new();
            map.insert(
                "max_file_size".to_string(),
                self.config.max_segment_size_bytes.to_string(),
            );
            Some(map)
        } else {
            None
        };

        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        // Setup callbacks for segment events
        let event_tx_start = self.event_tx.clone();
        let event_tx_complete = self.event_tx.clone();

        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx_start.try_send(event);
        });

        writer.set_on_segment_complete_callback(move |path, sequence, duration_secs| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_complete.try_send(event);
        });

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

        // Send first segment to writer
        if tx.send(Ok(first_segment)).await.is_err() {
            return Err(crate::Error::Other(
                "Writer channel closed unexpectedly".to_string(),
            ));
        }

        // Consume the rest of the HLS stream and send to writer
        let mut stream = std::pin::pin!(hls_stream);

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if self.cancellation_token.is_cancelled() || token.is_cancelled() {
                debug!(
                    "HLS download cancelled for {}",
                    self.config.streamer_id
                );
                break;
            }

            match result {
                Ok(hls_data) => {
                    // Send to writer
                    if tx.send(Ok(hls_data)).await.is_err() {
                        warn!("Writer channel closed, stopping HLS download");
                        break;
                    }
                }
                Err(e) => {
                    // Stream error - send error to writer and emit failure event
                    error!(
                        "HLS stream error for {}: {}",
                        self.config.streamer_id, e
                    );
                    let _ = tx.send(Err(PipelineError::Processing(e.to_string()))).await;
                    let _ = self
                        .event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: e.to_string(),
                            recoverable: true,
                        })
                        .await;
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
            Ok((items_written, files_created)) => {
                // Get final stats from writer state
                let stats = DownloadStats {
                    total_bytes: 0, // Will be updated from writer state when available
                    total_items: items_written,
                    total_duration_secs: 0.0, // Will be updated from writer state when available
                    files_created: files_created + 1,
                };

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
                    "HLS download completed for {}: {} items, {} files",
                    self.config.streamer_id, items_written, stats.files_created
                );

                Ok(stats)
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: e.to_string(),
                        recoverable: false,
                    })
                    .await;
                Err(crate::Error::Other(format!("HLS writer error: {}", e)))
            }
        }
    }
}
