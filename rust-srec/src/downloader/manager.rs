use super::engine::{
    ffmpeg::FfmpegDownloader, mesio::MesioDownloader, streamlink::StreamlinkDownloader,
    DownloadEngine, DownloadResult, DownloadTask,
};
use crate::{
    domain::{config::MergedConfig, engine::EngineType},
    metrics::ACTIVE_DOWNLOADS,
    pipeline::processor::{PipelineProcessor, PipelineProcessorImpl},
};
use crate::pipeline::action::{ActionType, RemuxActionConfig, UploadActionConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{error, info};

/// Manages download tasks, respecting concurrency limits.
pub struct DownloadManager {
    /// Limits the number of concurrent downloads.
    semaphore: Arc<Semaphore>,
    /// Processes downloaded files.
    pipeline_processor: Arc<dyn PipelineProcessor + Send + Sync>,
}

impl DownloadManager {
    /// Creates a new `DownloadManager` with a specified concurrency limit.
    pub fn new(max_concurrent_downloads: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent_downloads)),
            pipeline_processor: Arc::new(PipelineProcessorImpl),
        }
    }

    /// Submits a new download task to the manager.
    pub async fn submit_task(&self, task: DownloadTask) {
        let permit = self.semaphore.clone().acquire_owned().await.unwrap();
        ACTIVE_DOWNLOADS.inc();
        info!("Starting download task for {}", task.url);

        let engine = self.get_engine(&task.config);
        let result = engine.start(&task).await;

        match result {
            DownloadResult::Success(output_path) => {
                info!(
                    "Download task for {} finished successfully. Output: {:?}",
                    task.url, output_path
                );
                let pipeline_processor = self.pipeline_processor.clone();
                tokio::spawn(async move {
                    let remux_action = ActionType::Remux(RemuxActionConfig {
                        format: "mkv".to_string(),
                    });
                    let upload_action = ActionType::Upload(UploadActionConfig {
                        remote: "remote_server:/uploads".to_string(),
                    });
                    let pipeline = vec![remux_action, upload_action];
                    if let Some(output) = pipeline_processor.run(pipeline, output_path).await {
                        info!("Pipeline finished with output: {:?}", output);
                    } else {
                        error!("Pipeline failed for downloaded file.");
                    }
                });
            }
            DownloadResult::Failure(error_message) => {
                error!(
                    "Download task for {} failed: {}",
                    task.url, error_message
                );
            }
        }

        drop(permit);
        ACTIVE_DOWNLOADS.dec();
    }

    /// Selects the download engine based on the provided configuration.
    fn get_engine(&self, config: &MergedConfig) -> Arc<dyn DownloadEngine + Send + Sync> {
        let engine_type = match config.download_engine.as_str() {
            "ffmpeg" => EngineType::Ffmpeg,
            "streamlink" => EngineType::Streamlink,
            "mesio" => EngineType::Mesio,
            _ => {
                error!(
                    "Unknown download engine: {}. Defaulting to Ffmpeg.",
                    config.download_engine
                );
                EngineType::Ffmpeg
            }
        };

        match engine_type {
            EngineType::Ffmpeg => Arc::new(FfmpegDownloader),
            EngineType::Streamlink => Arc::new(StreamlinkDownloader),
            EngineType::Mesio => Arc::new(MesioDownloader),
        }
    }
}