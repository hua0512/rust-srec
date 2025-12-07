//! FFmpeg download engine implementation.

use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{DownloadEngine, DownloadHandle, DownloadProgress, EngineType, SegmentEvent};
use crate::Result;
use crate::database::models::engine::FfmpegEngineConfig;

/// FFmpeg-based download engine.
pub struct FfmpegEngine {
    /// Engine configuration.
    config: FfmpegEngineConfig,
    /// Cached version string.
    version: Option<String>,
}

impl FfmpegEngine {
    /// Create a new FFmpeg engine with default configuration.
    pub fn new() -> Self {
        Self::with_config(FfmpegEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: FfmpegEngineConfig) -> Self {
        let version = Self::detect_version(&config.binary_path);

        Self { config, version }
    }

    /// Detect ffmpeg version.
    fn detect_version(path: &str) -> Option<String> {
        std::process::Command::new(path)
            .arg("-version")
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.lines().next().map(|l| l.to_string()))
            })
    }

    /// Build ffmpeg command arguments.
    fn build_args(&self, handle: &DownloadHandle) -> Vec<String> {
        let config = &handle.config;
        let mut args = Vec::new();

        // 1. Force consistent output format
        args.extend(["-y".to_string(), "-hide_banner".to_string()]);

        // 2. Extra input arguments from config
        args.extend(self.config.input_args.clone());

        // 3. User Agent (if configured in engine or handle)
        // Handle config takes precedence if both set? Or engine config?
        // Usually engine config sets the default for the engine instance.
        if let Some(ref ua) = self.config.user_agent {
            args.extend(["-user_agent".to_string(), ua.clone()]);
        }

        // 4. Input options
        if let Some(ref proxy) = config.proxy_url {
            args.extend(["-http_proxy".to_string(), proxy.clone()]);
        }

        // Add headers
        for (key, value) in &config.headers {
            args.extend(["-headers".to_string(), format!("{}: {}", key, value)]);
        }

        // 5. Input URL
        args.extend(["-i".to_string(), config.url.clone()]);

        // 6. Output options
        args.extend(["-c".to_string(), "copy".to_string()]); // Copy streams without re-encoding

        // 7. Extra output arguments from config
        args.extend(self.config.output_args.clone());

        // Segment options if splitting is enabled
        if config.max_segment_duration_secs > 0 {
            args.extend([
                "-f".to_string(),
                "segment".to_string(),
                "-segment_time".to_string(),
                config.max_segment_duration_secs.to_string(),
                "-reset_timestamps".to_string(),
                "1".to_string(),
            ]);
        }

        // Output path
        let output_path = config.output_dir.join(format!(
            "{}.{}",
            config.filename_template, config.output_format
        ));

        if config.max_segment_duration_secs > 0 {
            // Use segment pattern
            let pattern = config.output_dir.join(format!(
                "{}_%03d.{}",
                config.filename_template, config.output_format
            ));
            args.push(pattern.to_string_lossy().to_string());
        } else {
            args.push(output_path.to_string_lossy().to_string());
        }

        args
    }

    /// Parse ffmpeg progress output.
    fn parse_progress(line: &str) -> Option<DownloadProgress> {
        // FFmpeg progress format: frame=X fps=X q=X size=XkB time=HH:MM:SS.ms bitrate=Xkbits/s speed=Xx
        if !line.starts_with("frame=") && !line.contains("size=") {
            return None;
        }

        let mut progress = DownloadProgress::default();

        // Parse size (format: "size=    1024kB" with possible leading spaces)
        if let Some(size_start) = line.find("size=") {
            let size_str = &line[size_start + 5..].trim_start();
            // Find where the number ends (at 'k' or 'K' for kB)
            if let Some(end) = size_str.find(|c: char| c == 'k' || c == 'K') {
                if let Ok(size) = size_str[..end].trim().parse::<u64>() {
                    // Size is in kB
                    progress.bytes_downloaded = size * 1024;
                }
            }
        }

        // Parse time
        if let Some(time_start) = line.find("time=") {
            let time_str = &line[time_start + 5..];
            if let Some(end) = time_str.find(' ') {
                let time_part = &time_str[..end];
                if let Some(duration) = Self::parse_time(time_part) {
                    progress.duration_secs = duration;
                }
            }
        }

        // Parse bitrate/speed
        if let Some(speed_start) = line.find("bitrate=") {
            let speed_str = &line[speed_start + 8..];
            if let Some(end) = speed_str.find("kbits/s") {
                if let Ok(bitrate) = speed_str[..end].trim().parse::<f64>() {
                    progress.speed_bytes_per_sec = (bitrate * 1024.0 / 8.0) as u64;
                }
            }
        }

        Some(progress)
    }

    /// Parse time string (HH:MM:SS.ms) to seconds.
    fn parse_time(time_str: &str) -> Option<f64> {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() != 3 {
            return None;
        }

        let hours: f64 = parts[0].parse().ok()?;
        let minutes: f64 = parts[1].parse().ok()?;
        let seconds: f64 = parts[2].parse().ok()?;

        Some(hours * 3600.0 + minutes * 60.0 + seconds)
    }
}

impl Default for FfmpegEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DownloadEngine for FfmpegEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Ffmpeg
    }

    async fn start(&self, handle: Arc<DownloadHandle>) -> Result<()> {
        let args = self.build_args(&handle);

        info!(
            "Starting ffmpeg download for streamer {} with args: {:?}",
            handle.config.streamer_id, args
        );

        // Spawn ffmpeg process
        let mut child = Command::new(&self.config.binary_path)
            .args(&args)
            .env("LC_ALL", "C") // Force consistent output
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| crate::Error::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| crate::Error::Other("Failed to capture ffmpeg stderr".to_string()))?;

        let event_tx = handle.event_tx.clone();
        let cancellation_token = handle.cancellation_token.clone();
        let streamer_id = handle.config.streamer_id.clone();

        // Spawn task to read ffmpeg output
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            let mut segment_index = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("FFmpeg download cancelled for {}", streamer_id);
                        break;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => {
                                // Check for segment completion
                                if line.contains("Opening") && line.contains("for writing") {
                                    segment_index += 1;
                                    debug!("FFmpeg segment {} started for {}", segment_index, streamer_id);
                                }

                                // Parse progress
                                if let Some(progress) = Self::parse_progress(&line) {
                                    total_bytes = progress.bytes_downloaded;
                                    total_duration = progress.duration_secs;

                                    let _ = event_tx.send(SegmentEvent::Progress(progress)).await;
                                }

                                // Check for errors
                                if line.contains("Error") || line.contains("error") {
                                    warn!("FFmpeg error for {}: {}", streamer_id, line);
                                }
                            }
                            Ok(None) => {
                                // EOF - process ended
                                debug!("FFmpeg process ended for {}", streamer_id);
                                break;
                            }
                            Err(e) => {
                                error!("Error reading ffmpeg output for {}: {}", streamer_id, e);
                                break;
                            }
                        }
                    }
                }
            }

            // Send completion event
            let _ = event_tx
                .send(SegmentEvent::DownloadCompleted {
                    total_bytes,
                    total_duration_secs: total_duration,
                    total_segments: segment_index,
                })
                .await;
        });

        // Wait for process to complete or cancellation
        let cancellation_token = handle.cancellation_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    // Kill the process
                    let _ = child.kill().await;
                }
                status = child.wait() => {
                    match status {
                        Ok(exit_status) => {
                            if !exit_status.success() {
                                warn!("FFmpeg exited with status: {}", exit_status);
                            }
                        }
                        Err(e) => {
                            error!("Error waiting for ffmpeg: {}", e);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        info!(
            "Stopping ffmpeg download for streamer {}",
            handle.config.streamer_id
        );
        handle.cancel();
        Ok(())
    }

    fn is_available(&self) -> bool {
        self.version.is_some()
    }

    fn version(&self) -> Option<String> {
        self.version.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time() {
        assert_eq!(FfmpegEngine::parse_time("00:00:10.50"), Some(10.5));
        assert_eq!(FfmpegEngine::parse_time("01:30:00.00"), Some(5400.0));
        assert_eq!(FfmpegEngine::parse_time("invalid"), None);
    }

    #[test]
    fn test_parse_progress() {
        let line = "frame=  100 fps=25 q=-1.0 size=    1024kB time=00:00:04.00 bitrate=2097.2kbits/s speed=1.00x";
        let progress = FfmpegEngine::parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 1024 * 1024);
        assert_eq!(p.duration_secs, 4.0);
    }

    #[test]
    fn test_engine_type() {
        let engine = FfmpegEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Ffmpeg);
    }
}
