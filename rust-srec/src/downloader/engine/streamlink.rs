//! Streamlink download engine implementation.

use async_trait::async_trait;
use chrono::Utc;
use pipeline_common::expand_filename_template;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadConfig, DownloadEngine, DownloadHandle, EngineType, SegmentEvent, SegmentInfo,
};
use super::utils::{
    OutputRecordReader, ensure_output_dir, is_segment_start, parse_opened_path, parse_progress,
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

        // Add Twitch-specific arguments (ttv-lol)
        if let Some(ref proxy) = self.config.twitch_proxy_playlist {
            args.extend(["--twitch-proxy-playlist".to_string(), proxy.clone()]);
        }

        if let Some(ref exclude) = self.config.twitch_proxy_playlist_exclude {
            args.extend([
                "--twitch-proxy-playlist-exclude".to_string(),
                exclude.clone(),
            ]);
        }

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
                "-strftime".to_string(),
                "1".to_string(), // Enable strftime expansion for %Y, %m, %d, etc. in filename
            ]);
        }

        // Output path (same logic as FFmpeg engine)
        let output_path = config.output_dir.join(format!(
            "{}.{}",
            config.filename_template, config.output_format
        ));

        if config.max_segment_duration_secs > 0 {
            // Use segment pattern with strftime enabled by -strftime 1 flag
            // Convert backslashes to forward slashes for FFmpeg compatibility on Windows
            let pattern_str = output_path.to_string_lossy().replace('\\', "/");
            args.push(pattern_str);
        } else {
            // Non-segment mode: manually expand strftime patterns
            // FFmpeg doesn't support -strftime flag in non-segment mode
            let expanded_template = expand_filename_template(&config.filename_template, None);
            let final_path = config
                .output_dir
                .join(format!("{}.{}", expanded_template, config.output_format));
            // Convert backslashes to forward slashes for FFmpeg compatibility on Windows
            let path_str = final_path.to_string_lossy().replace('\\', "/");
            args.push(path_str);
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
        // 1. Ensure output directory exists before spawning processes
        if let Err(e) = ensure_output_dir(&config.output_dir).await {
            let msg = e.to_string();
            let _ = handle.event_tx.try_send(SegmentEvent::DownloadFailed {
                error: msg.clone(),
                recoverable: false,
            });
            return Err(crate::Error::Other(msg));
        }

        let streamlink_args = self.build_streamlink_args(&config);
        let ffmpeg_args = self.build_ffmpeg_args(&config);
        let segment_mode = config.max_segment_duration_secs > 0;
        let single_output_path = if segment_mode {
            None
        } else {
            ffmpeg_args.last().map(|s| PathBuf::from(s.clone()))
        };

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

        let cancellation_token = handle.cancellation_token.clone();

        // 2. Spawn a waiter task for both processes.
        //
        // When cancellation is requested, the stdout pipe task stops and drops ffmpeg's stdin,
        // allowing ffmpeg to finalize and exit. We still report DownloadCompleted if ffmpeg exits 0.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<Option<i32>>();
        let cancellation_token_wait = cancellation_token.clone();
        tokio::spawn(async move {
            const STREAMLINK_KILL_TIMEOUT: Duration = Duration::from_secs(2);
            const FFMPEG_STOP_TIMEOUT: Duration = Duration::from_secs(10);

            // Ensure streamlink terminates promptly when cancellation is requested.
            tokio::select! {
                status = streamlink.wait() => {
                    if let Err(e) = status {
                        error!("Error waiting for streamlink process: {}", e);
                    }
                }
                _ = cancellation_token_wait.cancelled() => {
                    debug!("Stop requested, killing streamlink process");
                    let _ = streamlink.kill().await;
                    let _ = tokio::time::timeout(STREAMLINK_KILL_TIMEOUT, streamlink.wait()).await;
                }
            }

            let exit_code = match tokio::time::timeout(FFMPEG_STOP_TIMEOUT, ffmpeg.wait()).await {
                Ok(Ok(exit_status)) => exit_status.code(),
                Ok(Err(e)) => {
                    error!("Error waiting for ffmpeg process: {}", e);
                    Some(-1)
                }
                Err(_) => {
                    warn!("FFmpeg did not exit in time; killing process");
                    let _ = ffmpeg.kill().await;
                    match ffmpeg.wait().await {
                        Ok(exit_status) => exit_status.code(),
                        Err(e) => {
                            error!("Error waiting for killed ffmpeg process: {}", e);
                            Some(-1)
                        }
                    }
                }
            };

            let _ = exit_tx.send(exit_code);
        });

        let event_tx = handle.event_tx.clone();
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

        // 3. Spawn task to monitor ffmpeg stderr and emit events - waits for exit status
        let event_tx_clone = event_tx.clone();
        let streamer_id_clone = streamer_id.clone();
        tokio::spawn(async move {
            let mut reader = OutputRecordReader::new(ffmpeg_stderr);
            let mut active_segment: Option<(u32, PathBuf, f64)> = None;
            let mut next_segment_index = 0u32;
            let mut segments_completed = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;
            let mut last_seen_media_duration = 0.0f64;

            if let Some(path) = single_output_path {
                let index = 0u32;
                next_segment_index = 1;
                active_segment = Some((index, path.clone(), 0.0));
                let _ = event_tx_clone
                    .send(SegmentEvent::SegmentStarted {
                        path,
                        sequence: index,
                    })
                    .await;
            }

            loop {
                tokio::select! {
                    record_result = reader.next_record() => {
                        match record_result {
                            Ok(Some(line)) => {
                                // Check for segment completion using shared utility
                                if segment_mode
                                    && is_segment_start(&line)
                                    && let Some(path) = parse_opened_path(&line)
                                {
                                        // Complete the previous segment when a new one starts.
                                        if let Some((index, path, started_at)) = active_segment.take() {
                                            let size_bytes = tokio::fs::metadata(&path)
                                                .await
                                                .map(|m| m.len())
                                                .unwrap_or(0);
                                            let duration_secs = (last_seen_media_duration - started_at).max(0.0);
                                            segments_completed = segments_completed.saturating_add(1);
                                            let _ = event_tx_clone
                                                .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                                                    path,
                                                    duration_secs,
                                                    size_bytes,
                                                    index,
                                                    completed_at: Utc::now(),
                                                }))
                                                .await;
                                        }

                                        let index = next_segment_index;
                                        next_segment_index = next_segment_index.saturating_add(1);
                                        active_segment =
                                            Some((index, path.clone(), last_seen_media_duration));

                                        let _ = event_tx_clone
                                            .send(SegmentEvent::SegmentStarted { path, sequence: index })
                                            .await;
                                        debug!(
                                            "Segment {} started for {}",
                                            index, streamer_id_clone
                                        );
                                    }

                                // Parse progress using shared utility
                                if let Some(mut progress) = parse_progress(&line) {
                                    total_bytes = progress.bytes_downloaded;
                                    total_duration = progress.duration_secs;
                                    last_seen_media_duration = progress.media_duration_secs;

                                    progress.segments_completed = segments_completed;
                                    progress.current_segment = active_segment
                                        .as_ref()
                                        .map(|(_, p, _)| p.to_string_lossy().to_string());

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

            // Complete the last active segment (if any).
            if let Some((index, path, started_at)) = active_segment.take() {
                let size_bytes = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let duration_secs = (last_seen_media_duration - started_at).max(0.0);
                segments_completed = segments_completed.saturating_add(1);
                let _ = event_tx_clone
                    .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                        path,
                        duration_secs,
                        size_bytes,
                        index,
                        completed_at: Utc::now(),
                    }))
                    .await;
            }

            // Wait for exit status from process wait task
            let exit_code = exit_rx.await.ok().flatten();

            match exit_code {
                Some(0) => {
                    // Exit code 0 - success
                    let _ = event_tx_clone
                        .send(SegmentEvent::DownloadCompleted {
                            total_bytes,
                            total_duration_secs: total_duration,
                            total_segments: segments_completed,
                        })
                        .await;
                }
                Some(code) => {
                    // Non-zero exit code - failure
                    let _ = event_tx_clone
                        .send(SegmentEvent::DownloadFailed {
                            error: format!("Streamlink/FFmpeg exited with code {}", code),
                            recoverable: true,
                        })
                        .await;
                }
                None => {
                    let _ = event_tx_clone
                        .send(SegmentEvent::DownloadFailed {
                            error: "Streamlink/FFmpeg exited without an exit code".to_string(),
                            recoverable: true,
                        })
                        .await;
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
