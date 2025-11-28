//! Mesio native download engine implementation.

use async_trait::async_trait;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadEngine, DownloadHandle, DownloadProgress, EngineType, SegmentEvent, SegmentInfo,
};
use crate::Result;

/// Native Mesio download engine.
///
/// This engine uses native Rust for stream downloading with
/// zero-copy data handling via the Bytes crate. It supports
/// FLV and HLS formats directly.
pub struct MesioEngine {
    /// Whether the engine is available.
    available: bool,
    /// Engine version.
    version: String,
}

impl MesioEngine {
    /// Create a new Mesio engine.
    pub fn new() -> Self {
        Self {
            available: true, // Mesio is always available as it's a Rust crate
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Download an HLS stream.
    async fn download_hls(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
    ) -> Result<()> {
        let config = &handle.config;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| crate::Error::Other(format!("Failed to create HTTP client: {}", e)))?;

        // Add proxy if configured
        let client = if let Some(ref proxy_url) = config.proxy_url {
            reqwest::Client::builder()
                .proxy(
                    reqwest::Proxy::all(proxy_url)
                        .map_err(|e| crate::Error::Other(format!("Invalid proxy URL: {}", e)))?,
                )
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| crate::Error::Other(format!("Failed to create HTTP client: {}", e)))?
        } else {
            client
        };

        let mut segment_index = 0u32;
        let mut last_segment_url: Option<String> = None;

        loop {
            if handle.is_cancelled() {
                debug!("Mesio download cancelled for {}", config.streamer_id);
                break;
            }

            // Fetch the playlist
            let mut request = client.get(&config.url);
            
            // Add headers
            for (key, value) in &config.headers {
                request = request.header(key.as_str(), value.as_str());
            }

            // Add cookies
            if let Some(ref cookies) = config.cookies {
                request = request.header("Cookie", cookies.as_str());
            }

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    warn!("Failed to fetch playlist for {}: {}", config.streamer_id, e);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            let playlist_text = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    warn!("Failed to read playlist for {}: {}", config.streamer_id, e);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            // Parse HLS playlist and find new segments
            let segments = parse_hls_playlist(&playlist_text, &config.url);
            
            for segment_url in segments {
                if handle.is_cancelled() {
                    break;
                }

                // Skip already downloaded segments
                if let Some(ref last) = last_segment_url {
                    if &segment_url == last {
                        continue;
                    }
                }

                // Download segment
                let segment_data = match client.get(&segment_url).send().await {
                    Ok(resp) => match resp.bytes().await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("Failed to read segment data: {}", e);
                            continue;
                        }
                    },
                    Err(e) => {
                        warn!("Failed to download segment: {}", e);
                        continue;
                    }
                };

                // Write segment to file
                let segment_path = config.output_dir.join(format!(
                    "{}_{:03}.ts",
                    config.filename_template, segment_index
                ));

                if let Err(e) = write_segment(&segment_path, &segment_data).await {
                    error!("Failed to write segment: {}", e);
                    continue;
                }

                let segment_size = segment_data.len() as u64;
                progress.add_bytes(segment_size);
                progress.increment_segments();

                // Emit segment event
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                        path: segment_path,
                        duration_secs: 2.0, // Typical HLS segment duration
                        size_bytes: segment_size,
                        index: segment_index,
                        completed_at: Utc::now(),
                    }))
                    .await;

                segment_index += 1;
                last_segment_url = Some(segment_url);

                // Emit progress
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::Progress(progress.to_progress()))
                    .await;
            }

            // Wait before checking for new segments
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Download an FLV stream.
    async fn download_flv(
        &self,
        handle: Arc<DownloadHandle>,
        progress: Arc<DownloadProgressTracker>,
    ) -> Result<()> {
        let config = &handle.config;
        
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| crate::Error::Other(format!("Failed to create HTTP client: {}", e)))?;

        // Add proxy if configured
        let client = if let Some(ref proxy_url) = config.proxy_url {
            reqwest::Client::builder()
                .proxy(
                    reqwest::Proxy::all(proxy_url)
                        .map_err(|e| crate::Error::Other(format!("Invalid proxy URL: {}", e)))?,
                )
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| crate::Error::Other(format!("Failed to create HTTP client: {}", e)))?
        } else {
            client
        };

        let mut request = client.get(&config.url);
        
        // Add headers
        for (key, value) in &config.headers {
            request = request.header(key.as_str(), value.as_str());
        }

        // Add cookies
        if let Some(ref cookies) = config.cookies {
            request = request.header("Cookie", cookies.as_str());
        }

        let response = request
            .send()
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to connect to stream: {}", e)))?;

        let output_path = config.output_dir.join(format!(
            "{}.{}",
            config.filename_template, config.output_format
        ));

        let mut file = File::create(&output_path)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create output file: {}", e)))?;

        let mut stream = response.bytes_stream();
        use futures::StreamExt;

        let mut segment_index = 0u32;
        let mut segment_bytes = 0u64;
        let max_segment_size = if config.max_segment_size_bytes > 0 {
            config.max_segment_size_bytes
        } else {
            u64::MAX
        };

        while let Some(chunk_result) = stream.next().await {
            if handle.is_cancelled() {
                debug!("Mesio FLV download cancelled for {}", config.streamer_id);
                break;
            }

            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    warn!("Error reading stream chunk: {}", e);
                    continue;
                }
            };

            let chunk_len = chunk.len() as u64;
            
            if let Err(e) = file.write_all(&chunk).await {
                error!("Failed to write chunk: {}", e);
                break;
            }

            progress.add_bytes(chunk_len);
            segment_bytes += chunk_len;

            // Check if we need to split to a new segment
            if segment_bytes >= max_segment_size {
                file.flush().await.ok();
                
                // Emit segment completion
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                        path: output_path.clone(),
                        duration_secs: 0.0, // Unknown for FLV
                        size_bytes: segment_bytes,
                        index: segment_index,
                        completed_at: Utc::now(),
                    }))
                    .await;

                segment_index += 1;
                progress.increment_segments();
                segment_bytes = 0;

                // Create new file for next segment
                let new_path = config.output_dir.join(format!(
                    "{}_{:03}.{}",
                    config.filename_template, segment_index, config.output_format
                ));
                file = File::create(&new_path)
                    .await
                    .map_err(|e| crate::Error::Other(format!("Failed to create segment file: {}", e)))?;
            }

            // Emit progress periodically
            let _ = handle
                .event_tx
                .send(SegmentEvent::Progress(progress.to_progress()))
                .await;
        }

        file.flush().await.ok();

        Ok(())
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

        // Determine stream type from URL
        let is_hls = handle.config.url.contains(".m3u8") 
            || handle.config.url.contains("hls")
            || handle.config.output_format == "ts";

        // Spawn download task
        let download_result = if is_hls {
            self.download_hls(handle_clone.clone(), progress_clone).await
        } else {
            self.download_flv(handle_clone.clone(), progress_clone).await
        };

        // Send completion or failure event
        match download_result {
            Ok(()) => {
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: progress.total_bytes(),
                        total_duration_secs: progress.duration_secs(),
                        total_segments: progress.segments_completed(),
                    })
                    .await;
            }
            Err(e) => {
                error!("Mesio download failed for {}: {}", streamer_id, e);
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: e.to_string(),
                        recoverable: true,
                    })
                    .await;
            }
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
struct DownloadProgressTracker {
    bytes_downloaded: AtomicU64,
    segments_completed: AtomicU32,
    start_time: std::time::Instant,
}

impl DownloadProgressTracker {
    fn new() -> Self {
        Self {
            bytes_downloaded: AtomicU64::new(0),
            segments_completed: AtomicU32::new(0),
            start_time: std::time::Instant::now(),
        }
    }

    fn add_bytes(&self, bytes: u64) {
        self.bytes_downloaded.fetch_add(bytes, Ordering::SeqCst);
    }

    fn increment_segments(&self) {
        self.segments_completed.fetch_add(1, Ordering::SeqCst);
    }

    fn total_bytes(&self) -> u64 {
        self.bytes_downloaded.load(Ordering::SeqCst)
    }

    fn segments_completed(&self) -> u32 {
        self.segments_completed.load(Ordering::SeqCst)
    }

    fn duration_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    fn to_progress(&self) -> DownloadProgress {
        let bytes = self.total_bytes();
        let duration = self.duration_secs();
        let speed = if duration > 0.0 {
            (bytes as f64 / duration) as u64
        } else {
            0
        };

        DownloadProgress {
            bytes_downloaded: bytes,
            duration_secs: duration,
            speed_bytes_per_sec: speed,
            segments_completed: self.segments_completed(),
            current_segment: None,
        }
    }
}

/// Parse HLS playlist and extract segment URLs.
fn parse_hls_playlist(playlist: &str, base_url: &str) -> Vec<String> {
    let base = reqwest::Url::parse(base_url).ok();
    
    playlist
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter_map(|line| {
            if line.starts_with("http://") || line.starts_with("https://") {
                Some(line.to_string())
            } else if let Some(ref base) = base {
                base.join(line).ok().map(|u: reqwest::Url| u.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Write segment data to file.
async fn write_segment(path: &PathBuf, data: &[u8]) -> Result<()> {
    let mut file = File::create(path)
        .await
        .map_err(|e| crate::Error::Other(format!("Failed to create segment file: {}", e)))?;
    
    file.write_all(data)
        .await
        .map_err(|e| crate::Error::Other(format!("Failed to write segment data: {}", e)))?;
    
    file.flush()
        .await
        .map_err(|e| crate::Error::Other(format!("Failed to flush segment file: {}", e)))?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_hls_playlist() {
        let playlist = r#"#EXTM3U
#EXT-X-VERSION:3
#EXTINF:2.0,
segment001.ts
#EXTINF:2.0,
segment002.ts
#EXTINF:2.0,
https://example.com/segment003.ts
"#;
        
        let segments = parse_hls_playlist(playlist, "https://example.com/playlist.m3u8");
        assert_eq!(segments.len(), 3);
        assert!(segments[0].contains("segment001.ts"));
        assert!(segments[2].contains("segment003.ts"));
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
}
