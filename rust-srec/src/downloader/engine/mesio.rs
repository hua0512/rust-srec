use super::{DownloadEngine, DownloadResult, DownloadTask};
use crate::domain::engine::{EngineConfig, MesioConfig};
use async_trait::async_trait;
use futures::stream::StreamExt;
use mesio_engine::{
    config::DownloaderConfig,
    factory::{MesioDownloaderFactory, ProtocolType},
    hls::{config::HlsConfig, events::HlsStreamEvent},
    proxy::{ProxyConfig as MesioProxyConfig, ProxyType},
};
use std::{fs::File, io::Write, time::Duration as StdDuration};
use tokio::process::Child;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

pub struct MesioDownloader;

#[async_trait]
impl DownloadEngine for MesioDownloader {
    async fn start(&self, task: &DownloadTask) -> DownloadResult {
        let mut retries = 0;
        let policy = task.config.download_retry_policy.clone().unwrap_or_default();
        let mesio_config = match &task.engine_config {
            EngineConfig::Mesio(config) => config.clone(),
            _ => MesioConfig { timeout_ms: None },
        };

        loop {
            let mut downloader_config = DownloaderConfig::default();
            if let Some(timeout) = mesio_config.timeout_ms {
                downloader_config.timeout = StdDuration::from_millis(timeout);
            }
            if let Some(proxy) = &task.config.proxy_config {
                if proxy.enabled {
                    if let Some(url) = &proxy.url {
                        downloader_config.proxy = Some(MesioProxyConfig {
                            proxy_type: ProxyType::Http,
                            url: url.clone(),
                            auth: None,
                        });
                    }
                }
            }

            let hls_config = HlsConfig {
                base: downloader_config,
                ..Default::default()
            };

            let factory = MesioDownloaderFactory::new().with_hls_config(hls_config);

            let downloader = match factory.create_for_url(&task.url, ProtocolType::Auto).await {
                Ok(d) => d,
                Err(e) => {
                    error!("Failed to create mesio downloader for {}: {}", task.url, e);
                    return DownloadResult::Failure(format!(
                        "Failed to create mesio downloader: {}",
                        e
                    ));
                }
            };

            let stream = match downloader.download(&task.url).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Mesio download failed for {}: {}", task.url, e);
                    return DownloadResult::Failure(format!("Mesio download failed: {}", e));
                }
            };

            let output_path = task.output_path.clone();
            let mut output_file = match File::create(&output_path) {
                Ok(file) => file,
                Err(e) => {
                    return DownloadResult::Failure(format!(
                        "Failed to create output file {}: {}",
                        output_path.display(),
                        e
                    ));
                }
            };

            let result = match stream {
                mesio_engine::factory::DownloadStream::Flv(mut _flv_stream) => {
                    // We expect HLS, so this is an error
                    DownloadResult::Failure("Expected HLS stream, but got FLV".to_string())
                }
                mesio_engine::factory::DownloadStream::Hls(mut hls_stream) => {
                    let mut success = true;
                    while let Some(data_res) = hls_stream.next().await {
                        match data_res {
                            Ok(HlsStreamEvent::Data(data)) => {
                                if let Some(segment) = data.media_segment() {
                                    if let Err(e) = output_file.write_all(&segment.data) {
                                        error!("Failed to write to output file: {}", e);
                                        success = false;
                                        break;
                                    }
                                }
                            }
                            Ok(HlsStreamEvent::StreamEnded) => {
                                info!("Mesio stream ended for {}", task.url);
                                break;
                            }
                            Err(e) => {
                                error!("Error in HLS stream: {}", e);
                                success = false;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if success {
                        DownloadResult::Success(output_path)
                    } else {
                        DownloadResult::Failure("Failed to process HLS stream".to_string())
                    }
                }
            };

            if let DownloadResult::Success(_) = result {
                info!("Mesio download successful for {}", task.url);
                return result;
            }

            if retries >= policy.max_retries {
                error!(
                    "Mesio download for {} reached max retries ({})",
                    task.url, policy.max_retries
                );
                return DownloadResult::Failure(format!(
                    "Mesio download failed after {} retries",
                    policy.max_retries
                ));
            }

            retries += 1;
            let backoff_ms =
                (policy.delay_ms as f32 * policy.backoff_factor.powi(retries as i32 - 1)) as u64;
            warn!(
                "Retrying mesio download for {} in {}ms (attempt {}/{})",
                task.url, backoff_ms, retries, policy.max_retries
            );
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    fn stop(&self, _process: &mut Child) {
        warn!("Cannot stop a mesio download directly; it stops when the task is dropped.");
    }

    fn monitor(&self, _process: &Child) {
        info!("Monitoring for mesio is handled by its internal logging.");
    }
}