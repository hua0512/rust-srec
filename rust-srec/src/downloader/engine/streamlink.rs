use super::{DownloadEngine, DownloadResult, DownloadTask};
use crate::domain::engine::{EngineConfig, StreamlinkConfig};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

pub struct StreamlinkDownloader;

#[async_trait]
impl DownloadEngine for StreamlinkDownloader {
    async fn start(&self, task: &DownloadTask) -> DownloadResult {
        let mut retries = 0;
        let policy = task.config.download_retry_policy.clone().unwrap_or_default();

        loop {
            let mut command = self.build_command(task);
            info!("Executing streamlink command: {:?}", command);

            match command.spawn() {
                Ok(mut child) => {
                    let status = child.wait().await;
                    match status {
                        Ok(exit_status) if exit_status.success() => {
                            info!("Streamlink download successful for {}", task.url);
                            return DownloadResult::Success(task.output_path.clone());
                        }
                        Ok(exit_status) => {
                            error!(
                                "Streamlink download failed for {}: exit code {:?}",
                                task.url,
                                exit_status.code()
                            );
                        }
                        Err(e) => {
                            error!("Streamlink download failed for {}: {}", task.url, e);
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to spawn streamlink command for {}: {}",
                        task.url, e
                    );
                }
            }

            if retries >= policy.max_retries {
                error!(
                    "Streamlink download for {} reached max retries ({})",
                    task.url, policy.max_retries
                );
                return DownloadResult::Failure(format!(
                    "Streamlink download failed after {} retries",
                    policy.max_retries
                ));
            }

            retries += 1;
            let backoff_ms = (policy.delay_ms as f32 * policy.backoff_factor.powi(retries as i32 - 1)) as u64;
            warn!(
                "Retrying streamlink download for {} in {}ms (attempt {}/{})",
                task.url, backoff_ms, retries, policy.max_retries
            );
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    fn stop(&self, process: &mut Child) {
        info!("Stopping streamlink process (PID: {:?})", process.id());
        if let Err(e) = process.kill() {
            error!("Failed to kill streamlink process: {}", e);
        }
    }

    fn monitor(&self, process: &Child) {
        info!("Monitoring streamlink process (PID: {:?})", process.id());
    }
}

impl StreamlinkDownloader {
    fn build_command(&self, task: &DownloadTask) -> Command {
        let mut command = Command::new("streamlink");
        let streamlink_config = match &task.engine_config {
            EngineConfig::Streamlink(config) => config,
            _ => &StreamlinkConfig { default_args: None },
        };

        if let Some(args) = &streamlink_config.default_args {
            command.args(args.split_whitespace());
        }

        command
            .arg(&task.url)
            .arg("best")
            .arg("-o")
            .arg(&task.output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command
    }
}