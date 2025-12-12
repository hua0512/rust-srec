//! Streamlink download engine implementation.

use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{DownloadConfig, DownloadEngine, DownloadHandle, EngineType, SegmentEvent};
use super::utils::{
    ensure_output_dir, is_segment_start, parse_progress, spawn_piped_process_waiter,
};
use crate::Result;
use crate::database::models::engine::StreamlinkEngineConfig;

/// Streamlink-based download engine.
///
/// Streamlink is used for platforms that require special handling
/// or authentication. It pipes output to ffmpeg for remuxing.
pub struct StreamlinkEngine {
    /// Engine configuration.
    config: StreamlinkEngineConfig,
    /// Path to ffmpeg binary (for remuxing).
    ffmpeg_path: String,
    /// Cached version string.
    version: Option<String>,
}

impl StreamlinkEngine {
    /// Create a new Streamlink engine with default configuration.
    pub fn new() -> Self {
        Self::with_config(StreamlinkEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: StreamlinkEngineConfig) -> Self {
        let ffmpeg_path = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string());
        let version = Self::detect_version(&config.binary_path);

        Self {
            config,
            ffmpeg_path,
            version,
        }
    }

    /// Detect streamlink version.
    fn detect_version(path: &str) -> Option<String> {
        std::process::Command::new(path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            })
    }

    /// Build streamlink command arguments.
    fn build_streamlink_args(&self, config: &DownloadConfig) -> Vec<String> {
        let mut args = Vec::new();

        // Output to stdout for piping
        args.extend(["--stdout".to_string()]);

        // Add proxy if configured
        if let Some(ref proxy) = config.proxy_url {
            args.extend(["--http-proxy".to_string(), proxy.clone()]);
        }

        // Add cookies if configured
        if let Some(ref cookies) = config.cookies {
            args.extend(["--http-cookie".to_string(), cookies.clone()]);
        }

        // Add headers
        for (key, value) in &config.headers {
            args.extend(["--http-header".to_string(), format!("{}={}", key, value)]);
        }

        // Add extra arguments from config
        args.extend(self.config.extra_args.clone());

        // Stream URL must be the first positional argument followed by quality
        args.push(config.url.clone());

        // Quality selection (from config)
        args.push(self.config.quality.clone());

        args
    }

    /// Build ffmpeg command arguments for remuxing.
    fn build_ffmpeg_args(&self, config: &DownloadConfig) -> Vec<String> {
        let mut args = Vec::new();

        // Input from stdin
        args.extend([
            "-y".to_string(),
            "-hide_banner".to_string(),
            "-i".to_string(),
            "pipe:0".to_string(),
        ]);

        // Copy streams without re-encoding
        args.extend(["-c".to_string(), "copy".to_string()]);

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

            // Use segment pattern
            let pattern = config.output_dir.join(format!(
                "{}_%03d.{}",
                config.filename_template, config.output_format
            ));
            args.push(pattern.to_string_lossy().to_string());
        } else {
            // Single output file
            let output_path = config.output_dir.join(format!(
                "{}.{}",
                config.filename_template, config.output_format
            ));
            args.push(output_path.to_string_lossy().to_string());
        }

        args
    }

    /// Parse streamlink output for status information.
    fn parse_streamlink_output(line: &str) -> Option<StreamlinkStatus> {
        if line.contains("[cli][info] Stream ended") {
            return Some(StreamlinkStatus::StreamEnded);
        }
        if line.contains("[cli][info] Opening stream") {
            return Some(StreamlinkStatus::StreamOpened);
        }
        if line.contains("[cli][error]") {
            return Some(StreamlinkStatus::Error(line.to_string()));
        }
        if line.contains("error: ") {
            return Some(StreamlinkStatus::Error(line.to_string()));
        }
        None
    }
}

/// Status parsed from streamlink output.
#[derive(Debug)]
enum StreamlinkStatus {
    StreamOpened,
    StreamEnded,
    Error(String),
}

impl Default for StreamlinkEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DownloadEngine for StreamlinkEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Streamlink
    }

    async fn start(&self, handle: Arc<DownloadHandle>) -> Result<()> {
        let config = handle.config_snapshot();
        // 1. Ensure output directory exists before spawning processes (Requirements 2.1, 2.2)
        if let Err(e) = ensure_output_dir(&config.output_dir).await {
            let _ = handle.event_tx.try_send(SegmentEvent::DownloadFailed {
                error: e.clone(),
                recoverable: false,
            });
            return Err(crate::Error::Other(e));
        }

        let streamlink_args = self.build_streamlink_args(&config);
        let ffmpeg_args = self.build_ffmpeg_args(&config);

        info!(
            "Starting streamlink download for streamer {} with args: {:?}",
            config.streamer_id, streamlink_args
        );

        // Spawn streamlink process
        let mut streamlink = Command::new(&self.config.binary_path)
            .args(&streamlink_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| crate::Error::Other(format!("Failed to spawn streamlink: {}", e)))?;

        let mut streamlink_stdout = streamlink.stdout.take().ok_or_else(|| {
            crate::Error::Other("Failed to capture streamlink stdout".to_string())
        })?;
        let streamlink_stderr = streamlink.stderr.take().ok_or_else(|| {
            crate::Error::Other("Failed to capture streamlink stderr".to_string())
        })?;

        // Spawn ffmpeg process with stdin piped
        let mut ffmpeg = Command::new(&self.ffmpeg_path)
            .args(&ffmpeg_args)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| crate::Error::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

        let mut ffmpeg_stdin = ffmpeg
            .stdin
            .take()
            .ok_or_else(|| crate::Error::Other("Failed to capture ffmpeg stdin".to_string()))?;
        let ffmpeg_stderr = ffmpeg
            .stderr
            .take()
            .ok_or_else(|| crate::Error::Other("Failed to capture ffmpeg stderr".to_string()))?;

        // 2. Use shared piped process waiter utility (Requirements 3.1, 3.2)
        let exit_rx =
            spawn_piped_process_waiter(streamlink, ffmpeg, handle.cancellation_token.clone());

        let event_tx = handle.event_tx.clone();
        let cancellation_token = handle.cancellation_token.clone();
        let streamer_id = config.streamer_id.clone();

        // Spawn task to pipe streamlink stdout to ffmpeg stdin
        let cancellation_token_pipe = cancellation_token.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = [0u8; 8192];

            loop {
                tokio::select! {
                    _ = cancellation_token_pipe.cancelled() => {
                        break;
                    }
                    result = streamlink_stdout.read(&mut buffer) => {
                        match result {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                if ffmpeg_stdin.write_all(&buffer[..n]).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        // Spawn task to monitor streamlink stderr
        let streamer_id_clone = streamer_id.clone();
        let cancellation_token_clone = cancellation_token.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(streamlink_stderr);
            let mut lines = reader.lines();

            loop {
                tokio::select! {
                    _ = cancellation_token_clone.cancelled() => {
                        debug!("Streamlink stderr monitor cancelled for {}", streamer_id_clone);
                        break;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => {
                                if let Some(status) = Self::parse_streamlink_output(&line) {
                                    match status {
                                        StreamlinkStatus::StreamOpened => {
                                            info!("Streamlink stream opened for {}", streamer_id_clone);
                                        }
                                        StreamlinkStatus::StreamEnded => {
                                            info!("Streamlink stream ended for {}", streamer_id_clone);
                                        }
                                        StreamlinkStatus::Error(err) => {
                                            warn!("Streamlink error for {}: {}", streamer_id_clone, err);
                                        }
                                    }
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                error!("Error reading streamlink stderr: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        });

        // 3. Spawn task to monitor ffmpeg stderr and emit events - waits for exit status (Requirements 1.2, 1.3, 1.4)
        let event_tx_clone = event_tx.clone();
        let streamer_id_clone = streamer_id.clone();
        let cancellation_token_clone = cancellation_token.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(ffmpeg_stderr);
            let mut lines = reader.lines();
            let mut segment_index = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;
            let mut was_cancelled = false;

            loop {
                tokio::select! {
                    _ = cancellation_token_clone.cancelled() => {
                        debug!("FFmpeg stderr monitor cancelled for {}", streamer_id_clone);
                        was_cancelled = true;
                        break;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => {
                                // Check for segment completion using shared utility
                                if is_segment_start(&line) {
                                    segment_index += 1;
                                    debug!("Segment {} started for {}", segment_index, streamer_id_clone);
                                }

                                // Parse progress using shared utility
                                if let Some(progress) = parse_progress(&line) {
                                    total_bytes = progress.bytes_downloaded;
                                    total_duration = progress.duration_secs;
                                    let _ = event_tx_clone.send(SegmentEvent::Progress(progress)).await;
                                }
                            }
                            Ok(None) => {
                                debug!("FFmpeg process ended for {}", streamer_id_clone);
                                break;
                            }
                            Err(e) => {
                                error!("Error reading ffmpeg stderr: {}", e);
                                break;
                            }
                        }
                    }
                }
            }

            // If cancelled during reading, don't emit any event
            if was_cancelled {
                debug!(
                    "Download cancelled, not emitting completion event for {}",
                    streamer_id_clone
                );
                return;
            }

            // Wait for exit status from process wait task
            let exit_code = exit_rx.await.ok().flatten();

            match exit_code {
                Some(0) => {
                    // Exit code 0 - success (Requirement 1.3)
                    let _ = event_tx_clone
                        .send(SegmentEvent::DownloadCompleted {
                            total_bytes,
                            total_duration_secs: total_duration,
                            total_segments: segment_index,
                        })
                        .await;
                }
                Some(code) => {
                    // Non-zero exit code - failure (Requirements 1.2, 3.3)
                    let _ = event_tx_clone
                        .send(SegmentEvent::DownloadFailed {
                            error: format!("Streamlink/FFmpeg exited with code {}", code),
                            recoverable: true,
                        })
                        .await;
                }
                None => {
                    // Cancelled - don't emit any event (Requirement 1.4)
                    debug!(
                        "Download cancelled, not emitting completion event for {}",
                        streamer_id_clone
                    );
                }
            }
        });

        Ok(())
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        let streamer_id = handle.config_snapshot().streamer_id;
        info!("Stopping streamlink download for streamer {}", streamer_id);
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
    use crate::downloader::engine::utils::parse_time;

    #[test]
    fn test_engine_type() {
        let engine = StreamlinkEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Streamlink);
    }

    #[test]
    fn test_parse_streamlink_output() {
        assert!(matches!(
            StreamlinkEngine::parse_streamlink_output("[cli][info] Opening stream"),
            Some(StreamlinkStatus::StreamOpened)
        ));
        assert!(matches!(
            StreamlinkEngine::parse_streamlink_output("[cli][info] Stream ended"),
            Some(StreamlinkStatus::StreamEnded)
        ));
        assert!(matches!(
            StreamlinkEngine::parse_streamlink_output("[cli][error] Something went wrong"),
            Some(StreamlinkStatus::Error(_))
        ));
        assert!(StreamlinkEngine::parse_streamlink_output("random line").is_none());
    }

    #[test]
    fn test_parse_time() {
        // Tests now use shared utility
        assert_eq!(parse_time("00:00:10.50"), Some(10.5));
        assert_eq!(parse_time("01:30:00.00"), Some(5400.0));
        assert_eq!(parse_time("invalid"), None);
    }
}
