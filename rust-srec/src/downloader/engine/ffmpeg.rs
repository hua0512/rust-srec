use super::{DownloadEngine, DownloadResult, DownloadTask};
use crate::domain::engine::{EngineConfig, FfmpegConfig};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

pub struct FfmpegDownloader;

#[async_trait]
impl DownloadEngine for FfmpegDownloader {
    async fn start(&self, task: &DownloadTask) -> DownloadResult {
        let mut retries = 0;
        let policy = task.config.download_retry_policy.clone().unwrap_or_default();

        loop {
            let mut command = self.build_command(task);
            info!("Executing ffmpeg command: {:?}", command);

            match command.spawn() {
                Ok(mut child) => {
                    let status = child.wait().await;
                    match status {
                        Ok(exit_status) if exit_status.success() => {
                            info!("Ffmpeg download successful for {}", task.url);
                            return DownloadResult::Success(task.output_path.clone());
                        }
                        Ok(exit_status) => {
                            error!(
                                "Ffmpeg download failed for {}: exit code {:?}",
                                task.url,
                                exit_status.code()
                            );
                        }
                        Err(e) => {
                            error!("Ffmpeg download failed for {}: {}", task.url, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to spawn ffmpeg command for {}: {}", task.url, e);
                }
            }

            if retries >= policy.max_retries {
                error!(
                    "Ffmpeg download for {} reached max retries ({})",
                    task.url, policy.max_retries
                );
                return DownloadResult::Failure(format!(
                    "Ffmpeg download failed after {} retries",
                    policy.max_retries
                ));
            }

            retries += 1;
            let backoff_ms = (policy.delay_ms as f32 * policy.backoff_factor.powi(retries as i32 - 1)) as u64;
            warn!(
                "Retrying ffmpeg download for {} in {}ms (attempt {}/{})",
                task.url, backoff_ms, retries, policy.max_retries
            );
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    fn stop(&self, process: &mut Child) {
        info!("Stopping ffmpeg process (PID: {:?})", process.id());
        if let Err(e) = process.kill() {
            error!("Failed to kill ffmpeg process: {}", e);
        }
    }

    fn monitor(&self, process: &Child) {
        info!("Monitoring ffmpeg process (PID: {:?})", process.id());
        // TODO: Implement more sophisticated monitoring, e.g., parsing ffmpeg output
    }
}

impl FfmpegDownloader {
    fn build_command(&self, task: &DownloadTask) -> Command {
        let mut command = Command::new("ffmpeg");
        let ffmpeg_config = match &task.engine_config {
            EngineConfig::Ffmpeg(config) => config,
            _ => &FfmpegConfig { default_args: None },
        };

        if let Some(args) = &ffmpeg_config.default_args {
            command.args(args.split_whitespace());
        }

        command
            .arg("-i")
            .arg(&task.url)
            .arg("-c")
            .arg("copy")
            .arg(&task.output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command
    }
}