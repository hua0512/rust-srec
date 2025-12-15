//! FFmpeg download engine implementation.

use async_trait::async_trait;
use pipeline_common::expand_filename_template;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{DownloadConfig, DownloadEngine, DownloadHandle, EngineType, SegmentEvent};
use super::utils::{ensure_output_dir, is_segment_start, parse_progress, spawn_process_waiter};
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
    fn build_args(&self, config: &DownloadConfig) -> Vec<String> {
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
        // Build all headers into a single string
        let mut header_lines = Vec::new();

        for (key, value) in &config.headers {
            header_lines.push(format!("{}: {}", key, value));
        }

        // Add cookies as Cookie header if provided
        if let Some(ref cookies) = config.cookies {
            header_lines.push(format!("Cookie: {}", cookies));
        }

        // Only add -headers argument if there are headers to send
        if !header_lines.is_empty() {
            args.extend(["-headers".to_string(), header_lines.join("\r\n")]);
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
                "-strftime".to_string(),
                "1".to_string(), // Enable strftime expansion for %Y, %m, %d, etc. in filename
            ]);
        }

        // Output path
        let output_path = config.output_dir.join(format!(
            "{}.{}",
            config.filename_template, config.output_format
        ));

        if config.max_segment_duration_secs > 0 {
            // Use segment pattern with strftime enabled by -strftime 1 flag
            // In strftime mode, %d is the segment counter (not day-of-month)
            // TODO : ENSURE USER PATH IS VALID

            // Convert backslashes to forward slashes for FFmpeg compatibility on Windows
            // FFmpeg's segment muxer interprets backslashes as escape sequences
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
        let config = handle.config_snapshot();
        // 1. Ensure output directory exists before spawning process (Requirements 2.1, 2.2)
        if let Err(e) = ensure_output_dir(&config.output_dir).await {
            let _ = handle.event_tx.try_send(SegmentEvent::DownloadFailed {
                error: e.clone(),
                recoverable: false,
            });
            return Err(crate::Error::Other(e));
        }

        let args = self.build_args(&config);

        info!(
            "Starting ffmpeg download for streamer {} with args: {:?}",
            config.streamer_id, args
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

        // 2. Use shared process waiter utility (Requirements 3.1, 3.2)
        let exit_rx = spawn_process_waiter(child, handle.cancellation_token.clone());

        let event_tx = handle.event_tx.clone();
        let cancellation_token = handle.cancellation_token.clone();
        let streamer_id = config.streamer_id.clone();

        // 3. Spawn stderr reader task - waits for exit status before emitting event (Requirements 1.1, 1.3, 1.4)
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            let mut segment_index = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;
            let mut was_cancelled = false;

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("FFmpeg download cancelled for {}", streamer_id);
                        was_cancelled = true;
                        break;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => {
                                // Check for segment completion using shared utility
                                if is_segment_start(&line) {
                                    segment_index += 1;
                                    debug!("FFmpeg segment {} started for {}", segment_index, streamer_id);
                                }

                                // Parse progress using shared utility
                                if let Some(progress) = parse_progress(&line) {
                                    total_bytes = progress.bytes_downloaded;
                                    total_duration = progress.duration_secs;

                                    let _ = event_tx.send(SegmentEvent::Progress(progress)).await;
                                }

                                // Log all stderr output at debug level for troubleshooting
                                debug!("FFmpeg stderr for {}: {}", streamer_id, line);

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

            // If cancelled during reading, don't emit any event
            if was_cancelled {
                debug!(
                    "Download cancelled, not emitting completion event for {}",
                    streamer_id
                );
                return;
            }

            // Wait for exit status from process wait task
            let exit_code = exit_rx.await.ok().flatten();

            match exit_code {
                Some(0) => {
                    // Exit code 0 - success (Requirement 1.3)
                    let _ = event_tx
                        .send(SegmentEvent::DownloadCompleted {
                            total_bytes,
                            total_duration_secs: total_duration,
                            total_segments: segment_index,
                        })
                        .await;
                }
                Some(code) => {
                    // Non-zero exit code - failure (Requirements 1.1, 3.3)
                    let _ = event_tx
                        .send(SegmentEvent::DownloadFailed {
                            error: format!("FFmpeg exited with code {}", code),
                            recoverable: true,
                        })
                        .await;
                }
                None => {
                    // Cancelled - don't emit any event (Requirement 1.4)
                    debug!(
                        "Download cancelled, not emitting completion event for {}",
                        streamer_id
                    );
                }
            }
        });

        Ok(())
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        let streamer_id = handle.config_snapshot().streamer_id;
        info!("Stopping ffmpeg download for streamer {}", streamer_id);
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
    fn test_parse_time() {
        // Tests now use shared utility
        assert_eq!(parse_time("00:00:10.50"), Some(10.5));
        assert_eq!(parse_time("01:30:00.00"), Some(5400.0));
        assert_eq!(parse_time("invalid"), None);
    }

    #[test]
    fn test_parse_progress() {
        // Tests now use shared utility
        let line = "frame=  100 fps=25 q=-1.0 size=    1024kB time=00:00:04.00 bitrate=2097.2kbits/s speed=1.00x";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 1024 * 1024);
        assert_eq!(p.duration_secs, 4.0);
        // Verify media_duration_secs is populated from time= field
        assert_eq!(p.media_duration_secs, 4.0);
        // Verify playback_ratio is populated from speed= field
        assert_eq!(p.playback_ratio, 1.0);
    }

    #[test]
    fn test_parse_progress_with_different_speed() {
        // Test with speed=2.00x (downloading faster than real-time)
        let line = "frame=  200 fps=50 q=-1.0 size=    2048kB time=00:01:30.50 bitrate=1024.0kbits/s speed=2.00x";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 2048 * 1024);
        // 1 minute 30.5 seconds = 90.5 seconds
        assert_eq!(p.media_duration_secs, 90.5);
        assert_eq!(p.duration_secs, 90.5);
        assert_eq!(p.playback_ratio, 2.0);
    }

    #[test]
    fn test_parse_progress_without_speed() {
        // Some FFmpeg outputs may not include speed=
        let line = "frame=  100 fps=25 q=-1.0 size=    512kB time=00:00:10.00 bitrate=419.4kbits/s";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 512 * 1024);
        assert_eq!(p.media_duration_secs, 10.0);
        // playback_ratio should be 0.0 when speed= is not present
        assert_eq!(p.playback_ratio, 0.0);
    }

    #[test]
    fn test_engine_type() {
        let engine = FfmpegEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Ffmpeg);
    }
}
