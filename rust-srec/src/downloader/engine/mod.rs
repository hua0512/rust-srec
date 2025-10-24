use crate::domain::{config::MergedConfig, engine::EngineConfig};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::process::Child;

pub mod ffmpeg;
pub mod mesio;
pub mod streamlink;

/// Represents a download task, encapsulating all necessary information.
pub struct DownloadTask {
    pub url: String,
    pub output_path: PathBuf,
    pub config: MergedConfig,
    pub engine_config: EngineConfig,
}

/// Represents the result of a download operation.
pub enum DownloadResult {
    Success(PathBuf),
    Failure(String),
}

/// A trait that defines a unified interface for download engines.
#[async_trait]
pub trait DownloadEngine {
    /// Starts the download process.
    async fn start(&self, task: &DownloadTask) -> DownloadResult;

    /// Stops the download process.
    fn stop(&self, process: &mut Child);

    /// Monitors the download progress.
    fn monitor(&self, process: &Child);
}