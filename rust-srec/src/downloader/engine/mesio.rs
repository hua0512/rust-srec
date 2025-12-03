//! Mesio native download engine implementation.
//!
//! This engine uses the `mesio` crate's factory pattern for protocol detection
//! and stream downloading. It supports HLS and FLV formats through the
//! `MesioDownloaderFactory` API.
//!
//! HLS streams are written using `HlsWriter` from the `hls-fix` crate, which
//! provides proper file rotation, segment handling, and callback support for
//! emitting `SegmentEvent` notifications.
//!
//! FLV streams are written using `FlvWriter` from the `flv-fix` crate, which
//! provides similar functionality for FLV data.
//!
//! When `enable_processing` is set in the download config, streams are processed
//! through the respective pipelines (`HlsPipeline` or `FlvPipeline`) before writing.

use async_trait::async_trait;
use chrono::Utc;
use flv::data::FlvData;
use flv_fix::{FlvPipeline, FlvPipelineConfig, FlvWriter};
use futures::StreamExt;
use hls::HlsData;
use hls::segment::SegmentData;
use hls_fix::{HlsPipeline, HlsPipelineConfig, HlsWriter};
use mesio::flv::FlvProtocolConfig;
use mesio::flv::error::FlvDownloadError;
use mesio::proxy::{ProxyConfig, ProxyType};
use mesio::{
    DownloadStream, FlvProtocolBuilder, HlsProtocolBuilder, MesioDownloaderFactory, ProtocolType,
};

use pipeline_common::config::PipelineConfig;
use pipeline_common::{PipelineError, PipelineProvider, ProtocolWriter, StreamerContext};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadProgress, EngineType, SegmentEvent,
    SegmentInfo,
};
use crate::Result;

/// Native Mesio download engine.
///
/// This engine uses the `mesio` crate's `MesioDownloaderFactory` for
/// protocol detection and stream downloading. It supports HLS and FLV
/// formats with zero-copy data handling via the Bytes crate.
pub struct MesioEngine {
    /// Whether the engine is available.
    available: bool,
    /// Engine version.
    version: String,
    /// Default HLS configuration.
    hls_config: Option<mesio::hls::HlsConfig>,
    /// Default FLV configuration.
    flv_config: Option<FlvProtocolConfig>,
}

impl MesioEngine {
    /// Create a new Mesio engine with default configurations.
    pub fn new() -> Self {
        Self {
            available: true, // Mesio is always available as it's a Rust crate
            version: env!("CARGO_PKG_VERSION").to_string(),
            hls_config: Some(HlsProtocolBuilder::new().get_config()),
            flv_config: Some(FlvProtocolBuilder::new().get_config()),
        }
    }

    /// Create a new Mesio engine with custom HLS configuration built from HlsProtocolBuilder.
    pub fn with_hls_config(mut self, config: mesio::hls::HlsConfig) -> Self {
        self.hls_config = Some(config);
        self
    }

    /// Create a new Mesio engine with custom FLV configuration.
    pub fn with_flv_config(mut self, config: FlvProtocolConfig) -> Self {
        self.flv_config = Some(config);
        self
    }

    /// Detect the protocol type from a URL using the mesio factory.
    ///
    /// Returns the detected `ProtocolType` (HLS or FLV) based on URL patterns.
    pub fn detect_protocol(url: &str) -> Result<ProtocolType> {
        MesioDownloaderFactory::detect_protocol(url)
            .map_err(|e| crate::Error::Other(format!("Protocol detection failed: {}", e)))
    }

    /// Create a MesioDownloaderFactory with the given configuration and cancellation token.
    ///
    /// This method creates a factory configured with HLS and FLV settings mapped from
    /// the provided DownloadConfig, and associates it with the given cancellation token.
    pub fn create_factory(
        &self,
        config: &DownloadConfig,
        token: CancellationToken,
    ) -> MesioDownloaderFactory {
        let hls_config = build_hls_config(config);
        let flv_config = build_flv_config(config);

        MesioDownloaderFactory::new()
            .with_hls_config(hls_config)
            .with_flv_config(flv_config)
            .with_token(token)
    }

    /// Download an HLS stream using the mesio factory and HlsWriter.
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
    async fn download_hls(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
    ) -> Result<()> {
        let config = &handle.config;
        let token = CancellationToken::new();

        // Create factory with configuration
        let factory = self.create_factory(config, token.clone());

        // Create the HLS downloader instance
        let mut downloader = factory
            .create_for_url(&config.url, ProtocolType::Hls)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create HLS downloader: {}", e)))?;

        // Get the download stream
        let download_stream = downloader
            .download_with_sources(&config.url)
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
                let _ = handle
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
                let _ = handle
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
            config.streamer_id,
            extension.to_uppercase(),
            config.enable_processing
        );

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create HlsWriter with callbacks for segment events
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();
        let ext = extension.to_string();
        let event_tx = handle.event_tx.clone();
        let event_tx_close = handle.event_tx.clone();

        // Build extras for max_file_size if configured
        let extras = if config.max_segment_size_bytes > 0 {
            let mut map = HashMap::new();
            map.insert(
                "max_file_size".to_string(),
                config.max_segment_size_bytes.to_string(),
            );
            Some(map)
        } else {
            None
        };

        // Route based on enable_processing flag
        if config.enable_processing {
            // Process stream through HlsPipeline before writing
            self.download_hls_with_pipeline(
                handle,
                progress,
                token,
                hls_stream,
                first_segment,
                extension,
                output_dir,
                base_name,
                ext,
                extras,
                event_tx,
                event_tx_close,
                pipeline_config,
                channel_size,
            )
            .await
        } else {
            // Send stream data directly to HlsWriter (no pipeline processing)
            self.download_hls_raw(
                handle,
                progress,
                token,
                hls_stream,
                first_segment,
                output_dir,
                base_name,
                ext,
                extras,
                event_tx,
                event_tx_close,
                channel_size,
            )
            .await
        }
    }

    /// Download HLS stream with pipeline processing enabled.
    ///
    /// Routes the stream through HlsPipeline for defragmentation, segment splitting,
    /// and other processing before writing to HlsWriter.
    #[allow(clippy::too_many_arguments)]
    async fn download_hls_with_pipeline(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        extension: &str,
        output_dir: std::path::PathBuf,
        base_name: String,
        ext: String,
        extras: Option<HashMap<String, String>>,
        event_tx: tokio::sync::mpsc::Sender<SegmentEvent>,
        event_tx_close: tokio::sync::mpsc::Sender<SegmentEvent>,
        pipeline_config: pipeline_common::config::PipelineConfig,
        channel_size: usize,
    ) -> Result<()> {
        let config = &handle.config;

        info!(
            "Starting HLS download with pipeline processing for {}",
            config.streamer_id
        );

        // Build HLS pipeline configuration
        let hls_pipeline_config = config.build_hls_pipeline_config();

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
        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        // Register callback for segment start events
        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx.try_send(event);
        });

        // Register callback for segment complete events
        writer.set_on_segment_complete_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs: 0.0,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_close.try_send(event);
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
            if handle.is_cancelled() || token.is_cancelled() {
                debug!("HLS download cancelled for {}", handle.config.streamer_id);
                break;
            }

            match result {
                Ok(hls_data) => {
                    // Track bytes for progress
                    let byte_count = match &hls_data {
                        HlsData::TsData(ts) => ts.data.len() as u64,
                        HlsData::M4sData(m4s) => m4s.data().len() as u64,
                        HlsData::EndMarker => {
                            progress.increment_segments();
                            0
                        }
                    };

                    if byte_count > 0 {
                        progress.add_bytes(byte_count);
                    }

                    // Send to pipeline input
                    if pipeline_input_tx.send(Ok(hls_data)).await.is_err() {
                        warn!("Pipeline input channel closed, stopping HLS download");
                        break;
                    }

                    // Emit progress if threshold reached
                    if progress.should_emit_progress() {
                        let _ = handle
                            .event_tx
                            .send(SegmentEvent::Progress(progress.to_progress()))
                            .await;
                    }
                }
                Err(e) => {
                    error!("HLS stream error for {}: {}", handle.config.streamer_id, e);
                    let _ = pipeline_input_tx
                        .send(Err(PipelineError::Processing(e.to_string())))
                        .await;
                    let _ = handle
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
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: progress.total_bytes(),
                        total_duration_secs: progress.duration_secs(),
                        total_segments: files_created + 1,
                    })
                    .await;

                info!(
                    "HLS download with pipeline completed for {}: {} bytes, {} items, {} files",
                    handle.config.streamer_id,
                    progress.total_bytes(),
                    items_written,
                    files_created + 1
                );

                Ok(())
            }
            Err(e) => {
                let _ = handle
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
    #[allow(clippy::too_many_arguments)]
    async fn download_hls_raw(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        output_dir: std::path::PathBuf,
        base_name: String,
        ext: String,
        extras: Option<HashMap<String, String>>,
        event_tx: tokio::sync::mpsc::Sender<SegmentEvent>,
        event_tx_close: tokio::sync::mpsc::Sender<SegmentEvent>,
        channel_size: usize,
    ) -> Result<()> {
        info!(
            "Starting HLS download without pipeline processing for {}",
            handle.config.streamer_id
        );

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<HlsData, PipelineError>>(channel_size);

        // Create HlsWriter with callbacks
        let mut writer = HlsWriter::new(output_dir, base_name, ext, extras);

        // Register callback for segment start events
        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx.try_send(event);
        });

        // Register callback for segment complete events
        writer.set_on_segment_complete_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs: 0.0,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_close.try_send(event);
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
            if handle.is_cancelled() || token.is_cancelled() {
                debug!("HLS download cancelled for {}", handle.config.streamer_id);
                break;
            }

            match result {
                Ok(hls_data) => {
                    // Track bytes for progress
                    let byte_count = match &hls_data {
                        HlsData::TsData(ts) => ts.data.len() as u64,
                        HlsData::M4sData(m4s) => m4s.data().len() as u64,
                        HlsData::EndMarker => {
                            progress.increment_segments();
                            0
                        }
                    };

                    if byte_count > 0 {
                        progress.add_bytes(byte_count);
                    }

                    // Send to writer
                    if tx.send(Ok(hls_data)).await.is_err() {
                        warn!("Writer channel closed, stopping HLS download");
                        break;
                    }

                    // Emit progress if threshold reached
                    if progress.should_emit_progress() {
                        let _ = handle
                            .event_tx
                            .send(SegmentEvent::Progress(progress.to_progress()))
                            .await;
                    }
                }
                Err(e) => {
                    // Stream error - send error to writer and emit failure event
                    error!("HLS stream error for {}: {}", handle.config.streamer_id, e);
                    let _ = tx.send(Err(PipelineError::Processing(e.to_string()))).await;
                    let _ = handle
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
                // Emit completion event with stats from writer
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: progress.total_bytes(),
                        total_duration_secs: progress.duration_secs(),
                        total_segments: files_created + 1,
                    })
                    .await;

                info!(
                    "HLS download completed for {}: {} bytes, {} items, {} files",
                    handle.config.streamer_id,
                    progress.total_bytes(),
                    items_written,
                    files_created + 1
                );

                Ok(())
            }
            Err(e) => {
                let _ = handle
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

    /// Download an FLV stream using the mesio factory and FlvWriter.
    ///
    /// This method:
    /// 1. Creates a MesioDownloaderFactory with the configured FLV settings
    /// 2. Creates a DownloaderInstance::Flv using the factory
    /// 3. Calls download_with_sources() to get the FLV stream
    /// 4. If `enable_processing` is true, routes stream through FlvPipeline
    /// 5. Creates a FlvWriter with callbacks for segment events
    /// 6. Sends FlvData items to the writer via channel
    /// 7. Handles cancellation, progress tracking, and error reporting
    async fn download_flv(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
    ) -> Result<()> {
        let config = &handle.config;
        let token = CancellationToken::new();

        // Create factory with configuration
        let factory = self.create_factory(config, token.clone());

        // Create the FLV downloader instance
        let mut downloader = factory
            .create_for_url(&config.url, ProtocolType::Flv)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create FLV downloader: {}", e)))?;

        // Add the source URL
        downloader.add_source(&config.url, 0);

        // Get the download stream
        let download_stream = downloader
            .download_with_sources(&config.url)
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

        info!(
            "Starting FLV download for {}, processing_enabled: {}",
            config.streamer_id, config.enable_processing
        );

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create FlvWriter with callbacks for segment events
        let output_dir = config.output_dir.clone();
        let base_name = config.filename_template.clone();
        let event_tx = handle.event_tx.clone();
        let event_tx_close = handle.event_tx.clone();

        // Build extras for enable_low_latency
        let extras = {
            let mut map = HashMap::new();
            map.insert("enable_low_latency".to_string(), "true".to_string());
            Some(map)
        };

        // Route based on enable_processing flag
        if config.enable_processing {
            // Process stream through FlvPipeline before writing
            self.download_flv_with_pipeline(
                handle,
                progress,
                token,
                flv_stream,
                output_dir,
                base_name,
                extras,
                event_tx,
                event_tx_close,
                pipeline_config,
                channel_size,
            )
            .await
        } else {
            // Send stream data directly to FlvWriter (no pipeline processing)
            self.download_flv_raw(
                handle,
                progress,
                token,
                flv_stream,
                output_dir,
                base_name,
                extras,
                event_tx,
                event_tx_close,
                channel_size,
            )
            .await
        }
    }

    /// Download FLV stream with pipeline processing enabled.
    ///
    /// Routes the stream through FlvPipeline for defragmentation, GOP sorting,
    /// timing repair, and other processing before writing to FlvWriter.
    #[allow(clippy::too_many_arguments)]
    async fn download_flv_with_pipeline(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
        token: CancellationToken,
        flv_stream: impl futures::Stream<Item = std::result::Result<FlvData, FlvDownloadError>>
        + Send
        + Unpin,
        output_dir: std::path::PathBuf,
        base_name: String,
        extras: Option<HashMap<String, String>>,
        event_tx: tokio::sync::mpsc::Sender<SegmentEvent>,
        event_tx_close: tokio::sync::mpsc::Sender<SegmentEvent>,
        pipeline_config: pipeline_common::config::PipelineConfig,
        channel_size: usize,
    ) -> Result<()> {
        let config = &handle.config;

        info!(
            "Starting FLV download with pipeline processing for {}",
            config.streamer_id
        );

        // Build FLV pipeline configuration
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
        let mut writer = FlvWriter::new(output_dir, base_name, "flv".to_string(), extras);

        // Register callback for segment start events
        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx.try_send(event);
        });

        // Register callback for segment complete events
        writer.set_on_segment_complete_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs: 0.0,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_close.try_send(event);
        });

        // Spawn blocking writer task that reads from pipeline output
        let writer_task = tokio::task::spawn_blocking(move || writer.run(pipeline_output_rx));

        // Consume the FLV stream and send to pipeline
        let mut stream = std::pin::pin!(flv_stream);

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if handle.is_cancelled() || token.is_cancelled() {
                debug!("FLV download cancelled for {}", handle.config.streamer_id);
                break;
            }

            match result {
                Ok(flv_data) => {
                    // Track bytes for progress
                    let byte_count = flv_data.size() as u64;

                    if byte_count > 0 {
                        progress.add_bytes(byte_count);
                    }

                    // Send to pipeline input
                    if pipeline_input_tx.send(Ok(flv_data)).await.is_err() {
                        warn!("Pipeline input channel closed, stopping FLV download");
                        break;
                    }

                    // Emit progress if threshold reached
                    if progress.should_emit_progress() {
                        let _ = handle
                            .event_tx
                            .send(SegmentEvent::Progress(progress.to_progress()))
                            .await;
                    }
                }
                Err(e) => {
                    error!("FLV stream error for {}: {}", handle.config.streamer_id, e);
                    let _ = pipeline_input_tx
                        .send(Err(PipelineError::Processing(e.to_string())))
                        .await;
                    let _ = handle
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
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: progress.total_bytes(),
                        total_duration_secs: progress.duration_secs(),
                        total_segments: files_created + 1,
                    })
                    .await;

                info!(
                    "FLV download with pipeline completed for {}: {} bytes, {} items, {} files",
                    handle.config.streamer_id,
                    progress.total_bytes(),
                    items_written,
                    files_created + 1
                );

                Ok(())
            }
            Err(e) => {
                let _ = handle
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
    #[allow(clippy::too_many_arguments)]
    async fn download_flv_raw(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
        token: CancellationToken,
        flv_stream: impl futures::Stream<Item = std::result::Result<FlvData, FlvDownloadError>>
        + Send
        + Unpin,
        output_dir: std::path::PathBuf,
        base_name: String,
        extras: Option<HashMap<String, String>>,
        event_tx: tokio::sync::mpsc::Sender<SegmentEvent>,
        event_tx_close: tokio::sync::mpsc::Sender<SegmentEvent>,
        channel_size: usize,
    ) -> Result<()> {
        info!(
            "Starting FLV download without pipeline processing for {}",
            handle.config.streamer_id
        );

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<FlvData, PipelineError>>(channel_size);

        // Create FlvWriter with callbacks
        let mut writer = FlvWriter::new(output_dir, base_name, "flv".to_string(), extras);

        // Register callback for segment start events
        writer.set_on_segment_start_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentStarted {
                path: path.to_path_buf(),
                sequence,
            };
            let _ = event_tx.try_send(event);
        });

        // Register callback for segment complete events
        writer.set_on_segment_complete_callback(move |path, sequence| {
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: path.to_path_buf(),
                duration_secs: 0.0,
                size_bytes: 0,
                index: sequence,
                completed_at: Utc::now(),
            });
            let _ = event_tx_close.try_send(event);
        });

        // Spawn blocking writer task
        let writer_task = tokio::task::spawn_blocking(move || writer.run(rx));

        // Consume the FLV stream and send to writer
        let mut stream = std::pin::pin!(flv_stream);

        while let Some(result) = stream.next().await {
            // Check for cancellation
            if handle.is_cancelled() || token.is_cancelled() {
                debug!("FLV download cancelled for {}", handle.config.streamer_id);
                break;
            }

            match result {
                Ok(flv_data) => {
                    // Track bytes for progress
                    let byte_count = flv_data.size() as u64;

                    if byte_count > 0 {
                        progress.add_bytes(byte_count);
                    }

                    // Send to writer
                    if tx.send(Ok(flv_data)).await.is_err() {
                        warn!("Writer channel closed, stopping FLV download");
                        break;
                    }

                    // Emit progress if threshold reached
                    if progress.should_emit_progress() {
                        let _ = handle
                            .event_tx
                            .send(SegmentEvent::Progress(progress.to_progress()))
                            .await;
                    }
                }
                Err(e) => {
                    // Stream error - send error to writer and emit failure event
                    error!("FLV stream error for {}: {}", handle.config.streamer_id, e);
                    let _ = tx.send(Err(PipelineError::Processing(e.to_string()))).await;
                    let _ = handle
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
                // Emit completion event with stats from writer
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: progress.total_bytes(),
                        total_duration_secs: progress.duration_secs(),
                        total_segments: files_created + 1,
                    })
                    .await;

                info!(
                    "FLV download completed for {}: {} bytes, {} items, {} files",
                    handle.config.streamer_id,
                    progress.total_bytes(),
                    items_written,
                    files_created + 1
                );

                Ok(())
            }
            Err(e) => {
                let _ = handle
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

impl Default for MesioEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DownloadEngine for MesioEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Mesio
    }

    async fn start(&self, handle: Arc<DownloadHandle>) -> Result<()> {
        info!(
            "Starting mesio download for streamer {}",
            handle.config.streamer_id
        );

        let progress = Arc::new(DownloadProgressTracker::new());
        let progress_clone = progress.clone();
        let handle_clone = handle.clone();
        let streamer_id = handle.config.streamer_id.clone();

        // Detect protocol type using MesioDownloaderFactory
        let protocol_type = Self::detect_protocol(&handle.config.url)?;

        debug!(
            "Detected protocol {:?} for URL: {}",
            protocol_type, handle.config.url
        );

        // Execute download based on detected protocol type
        // Both methods use WriterTask and emit SegmentEvent::DownloadCompleted with WriterState stats
        // and SegmentEvent::DownloadFailed on errors.
        let download_result = match protocol_type {
            ProtocolType::Hls => {
                self.download_hls(handle_clone.clone(), progress_clone)
                    .await
            }
            ProtocolType::Flv => {
                self.download_flv(handle_clone.clone(), progress_clone)
                    .await
            }
            _ => {
                let error_msg = format!("Unsupported protocol type: {:?}", protocol_type);
                error!("{}", error_msg);
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: error_msg.clone(),
                        recoverable: false,
                    })
                    .await;
                return Err(crate::Error::Other(error_msg));
            }
        };

        // Log any errors but don't emit duplicate events
        // (Both HLS and FLV now emit their own events internally)
        if let Err(e) = &download_result {
            error!("Mesio download failed for {}: {}", streamer_id, e);
        }

        Ok(())
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        info!(
            "Stopping mesio download for streamer {}",
            handle.config.streamer_id
        );
        handle.cancel();
        Ok(())
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn version(&self) -> Option<String> {
        Some(self.version.clone())
    }
}

/// Progress tracker for downloads.
///
/// Tracks download statistics including bytes downloaded, segments completed,
/// and provides progress calculation methods with configurable emission intervals.
///
/// # Thread Safety
/// All counters use atomic operations for safe concurrent access.
pub struct DownloadProgressTracker {
    /// Total bytes downloaded (atomic for thread-safe updates).
    bytes_downloaded: AtomicU64,
    /// Number of segments completed (atomic for thread-safe updates).
    segments_completed: AtomicU32,
    /// Timestamp when tracking started.
    start_time: std::time::Instant,
    /// Bytes downloaded at last progress emission (for interval tracking).
    last_progress_bytes: AtomicU64,
    /// Timestamp of last progress emission in milliseconds since start.
    last_progress_time_ms: AtomicU64,
    /// Bytes threshold for progress emission (emit every N bytes).
    progress_interval_bytes: u64,
    /// Time threshold for progress emission in milliseconds.
    progress_interval_ms: u64,
}

impl DownloadProgressTracker {
    /// Default bytes threshold for progress emission (1 MB).
    pub const DEFAULT_PROGRESS_INTERVAL_BYTES: u64 = 1024 * 1024;
    /// Default time threshold for progress emission (1 second).
    pub const DEFAULT_PROGRESS_INTERVAL_MS: u64 = 1000;

    /// Create a new progress tracker with default intervals.
    ///
    /// Default intervals:
    /// - Bytes: 1 MB
    /// - Time: 1 second
    pub fn new() -> Self {
        Self {
            bytes_downloaded: AtomicU64::new(0),
            segments_completed: AtomicU32::new(0),
            start_time: std::time::Instant::now(),
            last_progress_bytes: AtomicU64::new(0),
            last_progress_time_ms: AtomicU64::new(0),
            progress_interval_bytes: Self::DEFAULT_PROGRESS_INTERVAL_BYTES,
            progress_interval_ms: Self::DEFAULT_PROGRESS_INTERVAL_MS,
        }
    }

    /// Create a new progress tracker with custom intervals.
    ///
    /// # Arguments
    /// * `bytes_interval` - Emit progress every N bytes downloaded
    /// * `time_interval_ms` - Emit progress every N milliseconds
    pub fn with_intervals(bytes_interval: u64, time_interval_ms: u64) -> Self {
        Self {
            bytes_downloaded: AtomicU64::new(0),
            segments_completed: AtomicU32::new(0),
            start_time: std::time::Instant::now(),
            last_progress_bytes: AtomicU64::new(0),
            last_progress_time_ms: AtomicU64::new(0),
            progress_interval_bytes: bytes_interval,
            progress_interval_ms: time_interval_ms,
        }
    }

    /// Add bytes to the download counter.
    ///
    /// Thread-safe: uses atomic fetch_add with SeqCst ordering.
    pub fn add_bytes(&self, bytes: u64) {
        self.bytes_downloaded.fetch_add(bytes, Ordering::SeqCst);
    }

    /// Increment the segment completion counter.
    ///
    /// Thread-safe: uses atomic fetch_add with SeqCst ordering.
    pub fn increment_segments(&self) {
        self.segments_completed.fetch_add(1, Ordering::SeqCst);
    }

    /// Get the total bytes downloaded.
    pub fn total_bytes(&self) -> u64 {
        self.bytes_downloaded.load(Ordering::SeqCst)
    }

    /// Get the number of segments completed.
    pub fn segments_completed(&self) -> u32 {
        self.segments_completed.load(Ordering::SeqCst)
    }

    /// Get the download duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Calculate the current download speed in bytes per second.
    ///
    /// Returns 0 if duration is 0 to avoid division by zero.
    pub fn speed_bytes_per_sec(&self) -> u64 {
        let duration = self.duration_secs();
        if duration > 0.0 {
            (self.total_bytes() as f64 / duration) as u64
        } else {
            0
        }
    }

    /// Check if progress should be emitted based on configured intervals.
    ///
    /// Progress is emitted when either:
    /// - Bytes downloaded since last emission exceeds `progress_interval_bytes`
    /// - Time since last emission exceeds `progress_interval_ms`
    ///
    /// If this returns `true`, the internal tracking state is updated.
    pub fn should_emit_progress(&self) -> bool {
        let current_bytes = self.total_bytes();
        let current_time_ms = self.start_time.elapsed().as_millis() as u64;

        let last_bytes = self.last_progress_bytes.load(Ordering::SeqCst);
        let last_time_ms = self.last_progress_time_ms.load(Ordering::SeqCst);

        let bytes_delta = current_bytes.saturating_sub(last_bytes);
        let time_delta_ms = current_time_ms.saturating_sub(last_time_ms);

        // Check if either threshold is exceeded
        if bytes_delta >= self.progress_interval_bytes || time_delta_ms >= self.progress_interval_ms
        {
            // Update last emission state
            self.last_progress_bytes
                .store(current_bytes, Ordering::SeqCst);
            self.last_progress_time_ms
                .store(current_time_ms, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Convert the current state to a `DownloadProgress` struct.
    pub fn to_progress(&self) -> DownloadProgress {
        DownloadProgress {
            bytes_downloaded: self.total_bytes(),
            duration_secs: self.duration_secs(),
            speed_bytes_per_sec: self.speed_bytes_per_sec(),
            segments_completed: self.segments_completed(),
            current_segment: None,
        }
    }
}

impl Default for DownloadProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}
// --- Configuration Mapping Functions ---

/// Build HLS configuration from rust-srec DownloadConfig using HlsProtocolBuilder.
///
/// Maps headers, cookies, and proxy settings from the download configuration
/// to the mesio HlsConfig structure using the builder pattern.
pub fn build_hls_config(config: &DownloadConfig) -> mesio::hls::HlsConfig {
    let mut builder = HlsProtocolBuilder::new();

    // Map headers
    for (key, value) in &config.headers {
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // Map proxy settings
    if let Some(ref proxy_url) = config.proxy_url {
        builder = builder.proxy(parse_proxy_url(proxy_url));
    }

    builder.get_config()
}

/// Build FLV configuration from rust-srec DownloadConfig using FlvProtocolBuilder.
///
/// Maps headers, cookies, and proxy settings from the download configuration
/// to the mesio FlvProtocolConfig structure using the builder pattern.
pub fn build_flv_config(config: &DownloadConfig) -> FlvProtocolConfig {
    let mut builder = FlvProtocolBuilder::new();

    // Map headers
    for (key, value) in &config.headers {
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // Map proxy settings using with_config since FlvProtocolBuilder doesn't have a proxy method
    if let Some(ref proxy_url) = config.proxy_url {
        let proxy = parse_proxy_url(proxy_url);
        builder = builder.with_config(|cfg| {
            cfg.base.proxy = Some(proxy);
        });
    }

    builder.get_config()
}

/// Build PipelineConfig from rust-srec DownloadConfig.
///
/// Maps max_file_size, max_duration, and channel_size settings from the download
/// configuration to the pipeline-common PipelineConfig structure.
///
/// If `pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, builds a new PipelineConfig from the individual settings.
pub fn build_pipeline_config(config: &DownloadConfig) -> PipelineConfig {
    if let Some(ref pipeline_config) = config.pipeline_config {
        pipeline_config.clone()
    } else {
        let mut builder = PipelineConfig::builder()
            .max_file_size(config.max_segment_size_bytes)
            .channel_size(64);

        if config.max_segment_duration_secs > 0 {
            builder = builder.max_duration(std::time::Duration::from_secs(
                config.max_segment_duration_secs,
            ));
        }

        builder.build()
    }
}

/// Build HlsPipelineConfig from rust-srec DownloadConfig.
///
/// If `hls_pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, returns the default HlsPipelineConfig.
pub fn build_hls_pipeline_config(config: &DownloadConfig) -> HlsPipelineConfig {
    config.hls_pipeline_config.clone().unwrap_or_default()
}

/// Build FlvPipelineConfig from rust-srec DownloadConfig.
///
/// If `flv_pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, returns the default FlvPipelineConfig.
pub fn build_flv_pipeline_config(config: &DownloadConfig) -> FlvPipelineConfig {
    config.flv_pipeline_config.clone().unwrap_or_default()
}

/// Parse a proxy URL string into a ProxyConfig.
///
/// Supports HTTP, HTTPS, and SOCKS5 proxy URLs.
/// Format: `[protocol://][user:pass@]host:port`
fn parse_proxy_url(url: &str) -> ProxyConfig {
    let url_lower = url.to_lowercase();

    // Determine proxy type from URL scheme
    let proxy_type = if url_lower.starts_with("socks5://") || url_lower.starts_with("socks5h://") {
        ProxyType::Socks5
    } else if url_lower.starts_with("https://") {
        ProxyType::Https
    } else {
        // Default to HTTP for http:// or no scheme
        ProxyType::Http
    };

    // Extract authentication if present (user:pass@host format)
    let auth = extract_proxy_auth(url);

    ProxyConfig {
        url: url.to_string(),
        proxy_type,
        auth,
    }
}

/// Extract authentication credentials from a proxy URL if present.
///
/// Looks for the pattern `user:pass@` in the URL.
fn extract_proxy_auth(url: &str) -> Option<mesio::proxy::ProxyAuth> {
    // Find the scheme separator
    let url_without_scheme = if let Some(pos) = url.find("://") {
        &url[pos + 3..]
    } else {
        url
    };

    // Check for @ which indicates auth credentials
    if let Some(at_pos) = url_without_scheme.find('@') {
        let auth_part = &url_without_scheme[..at_pos];
        if let Some(colon_pos) = auth_part.find(':') {
            let username = auth_part[..colon_pos].to_string();
            let password = auth_part[colon_pos + 1..].to_string();
            return Some(mesio::proxy::ProxyAuth { username, password });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_engine_type() {
        let engine = MesioEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Mesio);
    }

    #[test]
    fn test_is_available() {
        let engine = MesioEngine::new();
        assert!(engine.is_available());
    }

    #[test]
    fn test_progress_tracker() {
        let tracker = DownloadProgressTracker::new();
        tracker.add_bytes(1000);
        tracker.add_bytes(500);
        tracker.increment_segments();

        assert_eq!(tracker.total_bytes(), 1500);
        assert_eq!(tracker.segments_completed(), 1);
    }

    #[test]
    fn test_progress_tracker_with_custom_intervals() {
        let tracker = DownloadProgressTracker::with_intervals(500, 100);
        assert_eq!(tracker.progress_interval_bytes, 500);
        assert_eq!(tracker.progress_interval_ms, 100);
    }

    #[test]
    fn test_progress_tracker_duration() {
        let tracker = DownloadProgressTracker::new();
        // Duration should be non-negative
        assert!(tracker.duration_secs() >= 0.0);
    }

    #[test]
    fn test_progress_tracker_speed_calculation() {
        let tracker = DownloadProgressTracker::new();
        tracker.add_bytes(1000);
        // Speed should be non-negative
        assert!(tracker.speed_bytes_per_sec() >= 0);
    }

    #[test]
    fn test_progress_tracker_to_progress() {
        let tracker = DownloadProgressTracker::new();
        tracker.add_bytes(2000);
        tracker.increment_segments();
        tracker.increment_segments();

        let progress = tracker.to_progress();
        assert_eq!(progress.bytes_downloaded, 2000);
        assert_eq!(progress.segments_completed, 2);
        assert!(progress.duration_secs >= 0.0);
        assert!(progress.current_segment.is_none());
    }

    #[test]
    fn test_progress_tracker_should_emit_by_bytes() {
        // Create tracker with small byte threshold for testing
        let tracker = DownloadProgressTracker::with_intervals(100, 10000);

        // Initially should not emit (no bytes)
        assert!(!tracker.should_emit_progress());

        // Add bytes below threshold
        tracker.add_bytes(50);
        assert!(!tracker.should_emit_progress());

        // Add more bytes to exceed threshold
        tracker.add_bytes(60);
        assert!(tracker.should_emit_progress());

        // After emission, should not emit again until threshold exceeded
        assert!(!tracker.should_emit_progress());
    }

    #[test]
    fn test_progress_tracker_default_intervals() {
        let tracker = DownloadProgressTracker::new();
        assert_eq!(
            tracker.progress_interval_bytes,
            DownloadProgressTracker::DEFAULT_PROGRESS_INTERVAL_BYTES
        );
        assert_eq!(
            tracker.progress_interval_ms,
            DownloadProgressTracker::DEFAULT_PROGRESS_INTERVAL_MS
        );
    }

    #[test]
    fn test_progress_tracker_default_trait() {
        let tracker = DownloadProgressTracker::default();
        assert_eq!(tracker.total_bytes(), 0);
        assert_eq!(tracker.segments_completed(), 0);
    }

    // --- Configuration Mapping Tests ---

    fn create_test_download_config() -> DownloadConfig {
        DownloadConfig {
            url: "https://example.com/stream.m3u8".to_string(),
            output_dir: PathBuf::from("/tmp/downloads"),
            filename_template: "test-stream".to_string(),
            output_format: "ts".to_string(),
            max_segment_duration_secs: 0,
            max_segment_size_bytes: 0,
            proxy_url: None,
            cookies: None,
            headers: Vec::new(),
            streamer_id: "test-streamer".to_string(),
            session_id: "test-session".to_string(),
            enable_processing: false,
            pipeline_config: None,
            hls_pipeline_config: None,
            flv_pipeline_config: None,
        }
    }

    #[test]
    fn test_build_hls_config_default() {
        let config = create_test_download_config();
        let hls_config = build_hls_config(&config);

        // Should have default headers from mesio
        assert!(
            hls_config
                .base
                .headers
                .contains_key(reqwest::header::ACCEPT)
        );
        // Should not have proxy configured
        assert!(hls_config.base.proxy.is_none());
    }

    #[test]
    fn test_build_hls_config_with_headers() {
        let mut config = create_test_download_config();
        config.headers = vec![
            ("User-Agent".to_string(), "CustomAgent/1.0".to_string()),
            ("X-Custom-Header".to_string(), "custom-value".to_string()),
        ];

        let hls_config = build_hls_config(&config);

        // Check custom headers are mapped
        assert_eq!(
            hls_config
                .base
                .headers
                .get(reqwest::header::USER_AGENT)
                .map(|v| v.to_str().unwrap()),
            Some("CustomAgent/1.0")
        );
        assert_eq!(
            hls_config
                .base
                .headers
                .get("X-Custom-Header")
                .map(|v| v.to_str().unwrap()),
            Some("custom-value")
        );
    }

    #[test]
    fn test_build_hls_config_with_cookies() {
        let mut config = create_test_download_config();
        config.cookies = Some("session=abc123; token=xyz789".to_string());

        let hls_config = build_hls_config(&config);

        // Check cookies are mapped to Cookie header
        assert_eq!(
            hls_config
                .base
                .headers
                .get(reqwest::header::COOKIE)
                .map(|v| v.to_str().unwrap()),
            Some("session=abc123; token=xyz789")
        );
    }

    #[test]
    fn test_build_hls_config_with_proxy() {
        let mut config = create_test_download_config();
        config.proxy_url = Some("http://proxy.example.com:8080".to_string());

        let hls_config = build_hls_config(&config);

        // Check proxy is configured
        assert!(hls_config.base.proxy.is_some());
        let proxy = hls_config.base.proxy.unwrap();
        assert_eq!(proxy.url, "http://proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
    }

    #[test]
    fn test_build_flv_config_default() {
        let config = create_test_download_config();
        let flv_config = build_flv_config(&config);

        // Should have default headers from mesio
        assert!(
            flv_config
                .base
                .headers
                .contains_key(reqwest::header::ACCEPT)
        );
        // Should not have proxy configured
        assert!(flv_config.base.proxy.is_none());
    }

    #[test]
    fn test_build_flv_config_with_headers() {
        let mut config = create_test_download_config();
        config.headers = vec![("Referer".to_string(), "https://example.com".to_string())];

        let flv_config = build_flv_config(&config);

        // Check custom headers are mapped
        assert_eq!(
            flv_config
                .base
                .headers
                .get(reqwest::header::REFERER)
                .map(|v| v.to_str().unwrap()),
            Some("https://example.com")
        );
    }

    #[test]
    fn test_build_flv_config_with_cookies() {
        let mut config = create_test_download_config();
        config.cookies = Some("auth=secret".to_string());

        let flv_config = build_flv_config(&config);

        // Check cookies are mapped to Cookie header
        assert_eq!(
            flv_config
                .base
                .headers
                .get(reqwest::header::COOKIE)
                .map(|v| v.to_str().unwrap()),
            Some("auth=secret")
        );
    }

    #[test]
    fn test_parse_proxy_url_http() {
        let proxy = parse_proxy_url("http://proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
        assert_eq!(proxy.url, "http://proxy.example.com:8080");
        assert!(proxy.auth.is_none());
    }

    #[test]
    fn test_parse_proxy_url_https() {
        let proxy = parse_proxy_url("https://secure-proxy.example.com:443");
        assert_eq!(proxy.proxy_type, ProxyType::Https);
        assert_eq!(proxy.url, "https://secure-proxy.example.com:443");
    }

    #[test]
    fn test_parse_proxy_url_socks5() {
        let proxy = parse_proxy_url("socks5://socks-proxy.example.com:1080");
        assert_eq!(proxy.proxy_type, ProxyType::Socks5);
        assert_eq!(proxy.url, "socks5://socks-proxy.example.com:1080");
    }

    #[test]
    fn test_parse_proxy_url_with_auth() {
        let proxy = parse_proxy_url("http://user:password@proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
        assert!(proxy.auth.is_some());
        let auth = proxy.auth.unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "password");
    }

    #[test]
    fn test_parse_proxy_url_no_scheme() {
        // URLs without scheme should default to HTTP
        let proxy = parse_proxy_url("proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
    }

    #[test]
    fn test_extract_proxy_auth_with_credentials() {
        let auth = extract_proxy_auth("http://user:pass@host:8080");
        assert!(auth.is_some());
        let auth = auth.unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "pass");
    }

    #[test]
    fn test_extract_proxy_auth_without_credentials() {
        let auth = extract_proxy_auth("http://host:8080");
        assert!(auth.is_none());
    }

    #[test]
    fn test_create_factory() {
        let engine = MesioEngine::new();
        let config = create_test_download_config();
        let token = CancellationToken::new();

        // This should not panic and should create a valid factory
        let _factory = engine.create_factory(&config, token);
    }

    #[test]
    fn test_create_factory_with_full_config() {
        let engine = MesioEngine::new();
        let mut config = create_test_download_config();
        config.headers = vec![("User-Agent".to_string(), "TestAgent/1.0".to_string())];
        config.cookies = Some("session=test".to_string());
        config.proxy_url = Some("http://proxy:8080".to_string());

        let token = CancellationToken::new();
        let _factory = engine.create_factory(&config, token);
    }

    // --- Pipeline Configuration Mapping Tests ---

    #[test]
    fn test_build_pipeline_config_default() {
        let config = create_test_download_config();
        let pipeline_config = build_pipeline_config(&config);

        // Default values
        assert_eq!(pipeline_config.max_file_size, 0); // unlimited
        assert!(pipeline_config.max_duration.is_none());
        assert_eq!(pipeline_config.channel_size, 64);
    }

    #[test]
    fn test_build_pipeline_config_with_max_size() {
        let mut config = create_test_download_config();
        config.max_segment_size_bytes = 1024 * 1024 * 100; // 100 MB

        let pipeline_config = build_pipeline_config(&config);

        assert_eq!(pipeline_config.max_file_size, 1024 * 1024 * 100);
    }

    #[test]
    fn test_build_pipeline_config_with_max_duration() {
        let mut config = create_test_download_config();
        config.max_segment_duration_secs = 3600; // 1 hour

        let pipeline_config = build_pipeline_config(&config);

        assert!(pipeline_config.max_duration.is_some());
        assert_eq!(
            pipeline_config.max_duration.unwrap(),
            std::time::Duration::from_secs(3600)
        );
    }

    #[test]
    fn test_build_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        // Set explicit pipeline config
        config.pipeline_config = Some(
            PipelineConfig::builder()
                .max_file_size(500_000_000)
                .max_duration(std::time::Duration::from_secs(7200))
                .channel_size(128)
                .build(),
        );

        let pipeline_config = build_pipeline_config(&config);

        // Should use the explicit config, not build from individual fields
        assert_eq!(pipeline_config.max_file_size, 500_000_000);
        assert_eq!(
            pipeline_config.max_duration.unwrap(),
            std::time::Duration::from_secs(7200)
        );
        assert_eq!(pipeline_config.channel_size, 128);
    }

    #[test]
    fn test_build_hls_pipeline_config_default() {
        let config = create_test_download_config();
        let hls_pipeline_config = build_hls_pipeline_config(&config);

        // Default values
        assert!(hls_pipeline_config.defragment);
        assert!(hls_pipeline_config.split_segments);
        assert!(hls_pipeline_config.segment_limiter);
    }

    #[test]
    fn test_build_hls_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        config.hls_pipeline_config = Some(HlsPipelineConfig {
            defragment: false,
            split_segments: true,
            segment_limiter: false,
        });

        let hls_pipeline_config = build_hls_pipeline_config(&config);

        assert!(!hls_pipeline_config.defragment);
        assert!(hls_pipeline_config.split_segments);
        assert!(!hls_pipeline_config.segment_limiter);
    }

    #[test]
    fn test_build_flv_pipeline_config_default() {
        let config = create_test_download_config();
        let flv_pipeline_config = build_flv_pipeline_config(&config);

        // Default values
        assert!(flv_pipeline_config.duplicate_tag_filtering);
        assert!(flv_pipeline_config.enable_low_latency);
        assert!(!flv_pipeline_config.pipe_mode);
    }

    #[test]
    fn test_build_flv_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        config.flv_pipeline_config = Some(
            FlvPipelineConfig::builder()
                .duplicate_tag_filtering(false)
                .enable_low_latency(false)
                .pipe_mode(true)
                .build(),
        );

        let flv_pipeline_config = build_flv_pipeline_config(&config);

        assert!(!flv_pipeline_config.duplicate_tag_filtering);
        assert!(!flv_pipeline_config.enable_low_latency);
        assert!(flv_pipeline_config.pipe_mode);
    }
}
