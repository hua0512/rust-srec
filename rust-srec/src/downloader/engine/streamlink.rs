//! Streamlink download engine implementation.

use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{DownloadEngine, DownloadHandle, DownloadProgress, EngineType, SegmentEvent};
use crate::Result;

/// Streamlink-based download engine.
///
/// Streamlink is used for platforms that require special handling
/// or authentication. It pipes output to ffmpeg for remuxing.
pub struct StreamlinkEngine {
    /// Path to streamlink binary.
    streamlink_path: String,
    /// Path to ffmpeg binary (for remuxing).
    ffmpeg_path: String,
    /// Cached version string.
    version: Option<String>,
}

impl StreamlinkEngine {
    /// Create a new Streamlink engine.
    pub fn new() -> Self {
        let streamlink_path =
            std::env::var("STREAMLINK_PATH").unwrap_or_else(|_| "streamlink".to_string());
        let ffmpeg_path = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string());
        let version = Self::detect_version(&streamlink_path);

        Self {
            streamlink_path,
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
    fn build_streamlink_args(&self, handle: &DownloadHandle) -> Vec<String> {
        let config = &handle.config;
        let mut args = Vec::new();

        // Output to stdout for piping
        args.extend(["--stdout".to_string()]);

        // Quality selection (best by default)
        args.extend(["best".to_string()]);

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

        // Stream URL
        args.push(config.url.clone());

        args
    }

    /// Build ffmpeg command arguments for remuxing.
    fn build_ffmpeg_args(&self, handle: &DownloadHandle) -> Vec<String> {
        let config = &handle.config;
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
        let streamlink_args = self.build_streamlink_args(&handle);
        let ffmpeg_args = self.build_ffmpeg_args(&handle);

        info!(
            "Starting streamlink download for streamer {} with args: {:?}",
            handle.config.streamer_id, streamlink_args
        );

        // Spawn streamlink process
        let mut streamlink = Command::new(&self.streamlink_path)
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

        let event_tx = handle.event_tx.clone();
        let cancellation_token = handle.cancellation_token.clone();
        let streamer_id = handle.config.streamer_id.clone();

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

        // Spawn task to monitor ffmpeg stderr and emit events
        let event_tx_clone = event_tx.clone();
        let streamer_id_clone = streamer_id.clone();
        let cancellation_token_clone = cancellation_token.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(ffmpeg_stderr);
            let mut lines = reader.lines();
            let mut segment_index = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;

            loop {
                tokio::select! {
                    _ = cancellation_token_clone.cancelled() => {
                        debug!("FFmpeg stderr monitor cancelled for {}", streamer_id_clone);
                        break;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => {
                                // Check for segment completion
                                if line.contains("Opening") && line.contains("for writing") {
                                    segment_index += 1;
                                    debug!("Segment {} started for {}", segment_index, streamer_id_clone);
                                }

                                // Parse progress (same format as ffmpeg)
                                if line.contains("size=") && line.contains("time=") {
                                    if let Some(progress) = parse_ffmpeg_progress(&line) {
                                        total_bytes = progress.bytes_downloaded;
                                        total_duration = progress.duration_secs;
                                        let _ = event_tx_clone.send(SegmentEvent::Progress(progress)).await;
                                    }
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

            // Send completion event
            let _ = event_tx_clone
                .send(SegmentEvent::DownloadCompleted {
                    total_bytes,
                    total_duration_secs: total_duration,
                    total_segments: segment_index,
                })
                .await;
        });

        // Spawn task to wait for processes and handle cancellation
        let cancellation_token_clone = cancellation_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancellation_token_clone.cancelled() => {
                    // Kill both processes
                    let _ = streamlink.kill().await;
                    let _ = ffmpeg.kill().await;
                }
                _ = async {
                    let _ = streamlink.wait().await;
                    let _ = ffmpeg.wait().await;
                } => {}
            }
        });

        Ok(())
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        info!(
            "Stopping streamlink download for streamer {}",
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

/// Parse ffmpeg progress output (shared with FfmpegEngine).
fn parse_ffmpeg_progress(line: &str) -> Option<DownloadProgress> {
    if !line.contains("size=") {
        return None;
    }

    let mut progress = DownloadProgress::default();

    // Parse size
    if let Some(size_start) = line.find("size=") {
        let size_str = &line[size_start + 5..].trim_start();
        if let Some(end) = size_str.find(|c: char| c == 'k' || c == 'K') {
            if let Ok(size) = size_str[..end].trim().parse::<u64>() {
                progress.bytes_downloaded = size * 1024;
            }
        }
    }

    // Parse time
    if let Some(time_start) = line.find("time=") {
        let time_str = &line[time_start + 5..];
        if let Some(end) = time_str.find(' ') {
            let time_part = &time_str[..end];
            if let Some(duration) = parse_time(time_part) {
                progress.duration_secs = duration;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_time("00:00:10.50"), Some(10.5));
        assert_eq!(parse_time("01:30:00.00"), Some(5400.0));
        assert_eq!(parse_time("invalid"), None);
    }
}
