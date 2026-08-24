//! Streamlink download engine implementation.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pipeline_common::expand_filename_template;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::time::{Duration, Instant};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, EngineStartError,
    EngineType, SegmentEvent, SegmentInfo,
};
use super::utils::{
    OutputRecordReader, is_disk_full_line, is_segment_start, observe_segment_event_send,
    parse_opened_path, parse_progress,
};
use crate::database::models::engine::StreamlinkEngineConfig;

const PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const TASK_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(2);

fn build_http_cookie_args(cookie_string: &str) -> Vec<String> {
    // Streamlink expects repeated `--http-cookie name=value` arguments.
    cookie_string
        .split(&[';', '\n'][..])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim(), v.trim())))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .flat_map(|(k, v)| ["--http-cookie".to_string(), format!("{k}={v}")])
        .collect()
}

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
        let mut cmd = process_utils::std_command(path);
        cmd.arg("--version");
        cmd.output().ok().and_then(|output| {
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
            let parsed = build_http_cookie_args(cookies);
            if parsed.is_empty() {
                // Backward-compat: preserve previous behavior if parsing fails
                // (even though Streamlink may reject it).
                args.extend(["--http-cookie".to_string(), cookies.clone()]);
            } else {
                args.extend(parsed);
            }
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

enum StreamlinkPipelineExit {
    Ffmpeg(Option<i32>),
    Failed {
        kind: DownloadFailureKind,
        message: String,
    },
}

impl StreamlinkPipelineExit {
    fn with_cleanup_error(self, cleanup_error: String) -> Self {
        self.with_related_error("process cleanup error", cleanup_error)
    }

    fn with_secondary_error(self, secondary_error: String) -> Self {
        self.with_related_error("secondary process error", secondary_error)
    }

    fn with_related_error(self, label: &str, error: String) -> Self {
        match self {
            Self::Ffmpeg(Some(code)) if code != 0 => Self::Failed {
                kind: DownloadFailureKind::ProcessExit { code: Some(code) },
                message: format!("FFmpeg exited with code {code}; {label}: {error}"),
            },
            Self::Ffmpeg(_) => Self::Failed {
                kind: DownloadFailureKind::ProcessExit { code: None },
                message: format!("{label}: {error}"),
            },
            Self::Failed { kind, message } => Self::Failed {
                kind,
                message: format!("{message}; {label}: {error}"),
            },
        }
    }

    fn failure_summary(&self) -> Option<String> {
        match self {
            Self::Ffmpeg(Some(0)) => None,
            Self::Ffmpeg(Some(code)) => Some(format!("FFmpeg exited with code {code}")),
            Self::Ffmpeg(None) => Some("FFmpeg exited without an exit code".to_string()),
            Self::Failed { message, .. } => Some(message.clone()),
        }
    }
}

fn append_cleanup_result(message: &mut String, result: std::result::Result<Option<i32>, String>) {
    if let Err(cleanup_error) = result {
        message.push_str("; cleanup error: ");
        message.push_str(&cleanup_error);
    }
}

async fn terminate_and_reap(
    child: &mut Child,
    process_name: &str,
    timeout: Duration,
) -> std::result::Result<Option<i32>, String> {
    if let Err(error) = child.start_kill() {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code()),
            Ok(None) => {
                return Err(format!("failed to kill {process_name}: {error}"));
            }
            Err(wait_error) => {
                return Err(format!(
                    "failed to kill {process_name}: {error}; status check failed: {wait_error}"
                ));
            }
        }
    }

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status.code()),
        Ok(Err(error)) => Err(format!("failed to reap {process_name}: {error}")),
        Err(_) => Err(format!("timed out reaping {process_name}")),
    }
}

async fn wait_then_terminate(
    child: &mut Child,
    process_name: &str,
    timeout: Duration,
) -> (std::result::Result<Option<i32>, String>, bool) {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => (Ok(status.code()), true),
        Ok(Err(error)) => {
            let message = format!("failed to wait for {process_name}: {error}");
            match terminate_and_reap(child, process_name, timeout).await {
                Ok(_) => (Err(message), true),
                Err(cleanup_error) => (
                    Err(format!("{message}; cleanup error: {cleanup_error}")),
                    false,
                ),
            }
        }
        Err(_) => {
            warn!(
                process = process_name,
                "Process did not exit in time; killing it"
            );
            match terminate_and_reap(child, process_name, timeout).await {
                Ok(code) => (Ok(code), true),
                Err(error) => (
                    Err(format!(
                        "{process_name} did not exit within the stop timeout; cleanup error: {error}"
                    )),
                    false,
                ),
            }
        }
    }
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

    async fn run(&self, handle: Arc<DownloadHandle>) -> std::result::Result<(), EngineStartError> {
        let config = handle.config_snapshot();
        // Output directory is now prepared by
        // `DownloadManager::prepare_output_dir` before this method is called.
        // See the matching comment in ffmpeg.rs for the rationale.
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
        let mut streamlink_command = process_utils::tokio_command(&self.config.binary_path);
        streamlink_command
            .args(&streamlink_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut streamlink = streamlink_command.spawn().map_err(|e| {
            EngineStartError::new(
                DownloadFailureKind::Configuration,
                format!("Failed to spawn streamlink: {}", e),
            )
        })?;

        let mut streamlink_stdout = match streamlink.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let mut message = "Failed to capture streamlink stdout".to_string();
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(&mut streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT)
                        .await,
                );
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };
        let streamlink_stderr = match streamlink.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let mut message = "Failed to capture streamlink stderr".to_string();
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(&mut streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT)
                        .await,
                );
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };

        // Spawn ffmpeg process with stdin piped
        let mut ffmpeg_command = process_utils::tokio_command(&self.ffmpeg_path);
        crate::utils::configure_ffmpeg_locale(&mut ffmpeg_command);
        ffmpeg_command
            .args(&ffmpeg_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut ffmpeg = match ffmpeg_command.spawn() {
            Ok(ffmpeg) => ffmpeg,
            Err(error) => {
                let mut message = format!("Failed to spawn ffmpeg: {error}");
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(&mut streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT)
                        .await,
                );
                return Err(EngineStartError::new(
                    DownloadFailureKind::Configuration,
                    message,
                ));
            }
        };

        let mut ffmpeg_stdin = match ffmpeg.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let mut message = "Failed to capture ffmpeg stdin".to_string();
                let (streamlink_cleanup, ffmpeg_cleanup) = tokio::join!(
                    terminate_and_reap(&mut streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT,),
                    terminate_and_reap(&mut ffmpeg, "ffmpeg", PROCESS_CLEANUP_TIMEOUT),
                );
                append_cleanup_result(&mut message, streamlink_cleanup);
                append_cleanup_result(&mut message, ffmpeg_cleanup);
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };
        let ffmpeg_stderr = match ffmpeg.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let mut message = "Failed to capture ffmpeg stderr".to_string();
                let (streamlink_cleanup, ffmpeg_cleanup) = tokio::join!(
                    terminate_and_reap(&mut streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT,),
                    terminate_and_reap(&mut ffmpeg, "ffmpeg", PROCESS_CLEANUP_TIMEOUT),
                );
                append_cleanup_result(&mut message, streamlink_cleanup);
                append_cleanup_result(&mut message, ffmpeg_cleanup);
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };

        let cancellation_token = handle.cancellation_token.clone();
        let started_instant = Instant::now();
        let graceful_stop_timeout_secs = self.config.graceful_stop_timeout_secs;

        // 2. Spawn a waiter task for both processes.
        //
        // When cancellation is requested, the stdout pipe task stops and drops ffmpeg's stdin,
        // allowing ffmpeg to finalize and exit. We still report DownloadCompleted if ffmpeg exits 0.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<StreamlinkPipelineExit>();
        let cancellation_token_wait = cancellation_token.clone();
        let forced_settlement = CancellationToken::new();
        let process_forced_settlement = forced_settlement.clone();
        let pipe_failure = Arc::new(Mutex::new(None::<String>));
        let process_pipe_failure = pipe_failure.clone();
        let process_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let ffmpeg_stop_timeout = Duration::from_secs(graceful_stop_timeout_secs as u64);

            let (pipeline_exit, cleanup_confirmed) = tokio::select! {
                _ = cancellation_token_wait.cancelled() => {
                    debug!("Stop requested, killing streamlink process");
                    let streamlink_cleanup = terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                    ).await;
                    let (ffmpeg_result, ffmpeg_confirmed) =
                        wait_then_terminate(&mut ffmpeg, "ffmpeg", ffmpeg_stop_timeout).await;
                    let mut outcome = match ffmpeg_result {
                        Ok(code) => StreamlinkPipelineExit::Ffmpeg(code),
                        Err(message) => StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::ProcessExit { code: None },
                            message,
                        },
                    };
                    let streamlink_confirmed = match streamlink_cleanup {
                        Ok(_) => true,
                        Err(cleanup_error) => {
                            warn!(%cleanup_error, "Failed to stop streamlink cleanly");
                            outcome = outcome.with_cleanup_error(cleanup_error);
                            false
                        }
                    };
                    (outcome, streamlink_confirmed && ffmpeg_confirmed)
                }
                streamlink_result = streamlink.wait() => {
                    let (streamlink_failure, streamlink_confirmed) = match streamlink_result {
                        Ok(status) if status.success() => (None, true),
                        Ok(status) => (Some(StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::ProcessExit { code: status.code() },
                            message: format!("Streamlink exited with status {status}"),
                        }), true),
                        Err(error) => {
                            let failure = StreamlinkPipelineExit::Failed {
                                kind: DownloadFailureKind::ProcessExit { code: None },
                                message: format!("Failed to wait for Streamlink: {error}"),
                            };
                            match terminate_and_reap(
                                &mut streamlink,
                                "streamlink",
                                PROCESS_CLEANUP_TIMEOUT,
                            ).await {
                                Ok(_) => (Some(failure), true),
                                Err(cleanup_error) => (
                                    Some(failure.with_cleanup_error(cleanup_error)),
                                    false,
                                ),
                            }
                        }
                    };
                    let (ffmpeg_result, ffmpeg_confirmed) = wait_then_terminate(
                        &mut ffmpeg,
                        "ffmpeg",
                        ffmpeg_stop_timeout,
                    ).await;
                    let ffmpeg_outcome = match ffmpeg_result {
                        Ok(code) => StreamlinkPipelineExit::Ffmpeg(code),
                        Err(message) => StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::ProcessExit { code: None },
                            message,
                        },
                    };

                    let outcome = if let Some(mut failure) = streamlink_failure {
                        if let Some(secondary_error) = ffmpeg_outcome.failure_summary() {
                            failure = failure.with_secondary_error(secondary_error);
                        }
                        failure
                    } else if let Some(message) = process_pipe_failure.lock().clone() {
                        let mut failure = StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::Network,
                            message,
                        };
                        if let Some(secondary_error) = ffmpeg_outcome.failure_summary() {
                            failure = failure.with_secondary_error(secondary_error);
                        }
                        failure
                    } else {
                        ffmpeg_outcome
                    };
                    (outcome, streamlink_confirmed && ffmpeg_confirmed)
                }
                ffmpeg_result = ffmpeg.wait() => {
                    let (mut ffmpeg_outcome, ffmpeg_confirmed) = match ffmpeg_result {
                        Ok(status) if status.success() => (StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::Other,
                            message: "FFmpeg exited before Streamlink completed".to_string(),
                        }, true),
                        Ok(status) => (StreamlinkPipelineExit::Ffmpeg(status.code()), true),
                        Err(error) => {
                            let failure = StreamlinkPipelineExit::Failed {
                                kind: DownloadFailureKind::ProcessExit { code: None },
                                message: format!("Failed to wait for FFmpeg: {error}"),
                            };
                            match terminate_and_reap(
                                &mut ffmpeg,
                                "ffmpeg",
                                ffmpeg_stop_timeout,
                            ).await {
                                Ok(_) => (failure, true),
                                Err(cleanup_error) => (
                                    failure.with_cleanup_error(cleanup_error),
                                    false,
                                ),
                            }
                        }
                    };
                    let streamlink_confirmed = match terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                    ).await {
                        Ok(_) => true,
                        Err(cleanup_error) => {
                            warn!(%cleanup_error, "Failed to stop Streamlink after FFmpeg exited");
                            ffmpeg_outcome = ffmpeg_outcome.with_cleanup_error(cleanup_error);
                            false
                        }
                    };

                    let outcome = if let Some(message) = process_pipe_failure.lock().clone() {
                        let mut failure = StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::Network,
                            message,
                        };
                        if let Some(secondary_error) = ffmpeg_outcome.failure_summary() {
                            failure = failure.with_secondary_error(secondary_error);
                        }
                        failure
                    } else {
                        ffmpeg_outcome
                    };
                    (outcome, streamlink_confirmed && ffmpeg_confirmed)
                }
            };

            if !cleanup_confirmed {
                process_forced_settlement.cancel();
            }
            let cleanup_error = if cleanup_confirmed {
                None
            } else {
                pipeline_exit
                    .failure_summary()
                    .or_else(|| Some("streamlink pipeline cleanup was not confirmed".to_string()))
            };
            if exit_tx.send(pipeline_exit).is_err() {
                debug!("Download exit receiver dropped before streamlink pipeline completed");
            }
            (cleanup_confirmed, cleanup_error)
        }));

        let event_tx = handle.event_tx.clone();
        let streamer_id = config.streamer_id.clone();

        // Spawn task to pipe streamlink stdout to ffmpeg stdin
        let cancellation_token_pipe = cancellation_token.clone();
        let pipe_forced_settlement = forced_settlement.clone();
        let writer_pipe_failure = pipe_failure;
        let pipe_task = AbortOnDropHandle::new(tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = [0u8; 8192];

            loop {
                tokio::select! {
                    _ = cancellation_token_pipe.cancelled() => {
                        break;
                    }
                    _ = pipe_forced_settlement.cancelled() => {
                        warn!("Stopping Streamlink stdout pipe after unconfirmed process cleanup");
                        break;
                    }
                    result = streamlink_stdout.read(&mut buffer) => {
                        match result {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                if let Err(error) = ffmpeg_stdin.write_all(&buffer[..n]).await {
                                    *writer_pipe_failure.lock() = Some(format!(
                                        "Failed to pipe Streamlink output into FFmpeg: {error}"
                                    ));
                                    break;
                                }
                            }
                            Err(error) => {
                                *writer_pipe_failure.lock() = Some(format!(
                                    "Failed to read Streamlink output: {error}"
                                ));
                                break;
                            }
                        }
                    }
                }
            }
        }));

        // Spawn task to monitor streamlink stderr
        let streamer_id_clone = streamer_id.clone();
        let cancellation_token_clone = cancellation_token.clone();
        let stderr_forced_settlement = forced_settlement.clone();
        let streamlink_stderr_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let reader = BufReader::new(streamlink_stderr);
            let mut lines = reader.lines();

            loop {
                tokio::select! {
                    _ = cancellation_token_clone.cancelled() => {
                        debug!("Streamlink stderr monitor cancelled for {}", streamer_id_clone);
                        break;
                    }
                    _ = stderr_forced_settlement.cancelled() => {
                        warn!(
                            streamer_id = %streamer_id_clone,
                            "Stopping Streamlink stderr processing after unconfirmed process cleanup"
                        );
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
        }));

        // 3. Spawn task to monitor ffmpeg stderr and emit events - waits for exit status
        let event_tx_clone = event_tx.clone();
        let streamer_id_clone = streamer_id.clone();
        let output_dir_clone = config.output_dir.clone();
        let event_forced_settlement = forced_settlement.clone();
        let event_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let mut reader = OutputRecordReader::new(ffmpeg_stderr);
            let mut active_segment: Option<(u32, PathBuf, f64, DateTime<Utc>)> = None;
            let mut next_segment_index = 0u32;
            let mut segments_completed = 0u32;
            let mut total_bytes = 0u64;
            let mut total_duration = 0.0f64;
            // Set once when a disk-full signature is detected in stderr so a
            // later ProcessExit path doesn't double-emit DiskFull. See the
            // matching comment in ffmpeg.rs for the rationale.
            let mut disk_full_reported = false;
            let mut bytes_completed = 0u64;
            let mut media_duration_offset_secs = 0.0f64;
            let mut media_duration_total_secs = 0.0f64;
            let mut cached_active_segment_bytes = 0u64;
            let mut has_active_segment_fs_bytes = false;
            let mut last_active_segment_stat_at = Instant::now();
            let mut last_progress_snapshot: Option<(u64, f64, f64)> = None;
            let mut cleanup_unconfirmed = false;

            if let Some(path) = single_output_path {
                let index = 0u32;
                next_segment_index = 1;
                let started_at = Utc::now();
                active_segment = Some((index, path.clone(), 0.0, started_at));
                observe_segment_event_send(
                    event_tx_clone
                        .send(SegmentEvent::SegmentStarted {
                            path,
                            sequence: index,
                            started_at,
                        })
                        .await,
                    &streamer_id_clone,
                );
            }

            loop {
                tokio::select! {
                    biased;
                    _ = event_forced_settlement.cancelled() => {
                        cleanup_unconfirmed = true;
                        warn!(
                            streamer_id = %streamer_id_clone,
                            "Stopping FFmpeg stderr processing after unconfirmed Streamlink pipeline cleanup"
                        );
                        break;
                    }
                    record_result = reader.next_record() => {
                        match record_result {
                            Ok(Some(line)) => {
                                // Check for segment completion using shared utility
                                if segment_mode
                                    && is_segment_start(&line)
                                    && let Some(path) = parse_opened_path(&line)
                                {
                                        // Complete the previous segment when a new one starts.
                                        if let Some((index, path, started_media_at, started_at)) = active_segment.take() {
                                            let size_bytes = tokio::fs::metadata(&path)
                                                .await
                                                .map(|m| m.len())
                                                .unwrap_or(0);
                                            let duration_secs =
                                                (media_duration_total_secs - started_media_at).max(0.0);
                                            segments_completed = segments_completed.saturating_add(1);
                                            bytes_completed = bytes_completed.saturating_add(size_bytes);
                                            media_duration_offset_secs += duration_secs;
                                            media_duration_total_secs = media_duration_offset_secs;
                                            total_bytes = bytes_completed;
                                            total_duration = media_duration_offset_secs;
                                            cached_active_segment_bytes = 0;
                                            observe_segment_event_send(
                                                event_tx_clone
                                                    .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                                                        path,
                                                        duration_secs,
                                                        size_bytes,
                                                        index,
                                                        started_at: Some(started_at),
                                                        completed_at: Utc::now(),
                                                        split_reason_code: None,
                                                        split_reason_details_json: None,
                                                    }))
                                                    .await,
                                                &streamer_id_clone,
                                            );
                                        }

                                        let index = next_segment_index;
                                        next_segment_index = next_segment_index.saturating_add(1);
                                        let started_at = Utc::now();
                                        active_segment = Some((
                                            index,
                                            path.clone(),
                                            media_duration_total_secs,
                                            started_at,
                                        ));

                                        observe_segment_event_send(
                                            event_tx_clone
                                                .send(SegmentEvent::SegmentStarted {
                                                    path,
                                                    sequence: index,
                                                    started_at,
                                                })
                                                .await,
                                            &streamer_id_clone,
                                        );
                                        debug!(
                                            "Segment {} started for {}",
                                            index, streamer_id_clone
                                        );
                                    }

                                // Parse progress using shared utility
                                if let Some(mut progress) = parse_progress(&line) {
                                    let elapsed_secs = started_instant.elapsed().as_secs_f64();

                                    let segment_media_secs = progress.media_duration_secs;
                                    if segment_mode {
                                        media_duration_total_secs =
                                            media_duration_offset_secs + segment_media_secs;
                                    } else {
                                        media_duration_total_secs = segment_media_secs;
                                    }

                                    // Prefer filesystem-backed byte counts since FFmpeg's `size=`
                                    // can reset or be absent when segmenting.
                                    let mut bytes_total = progress.bytes_downloaded;
                                    if let Some((_, path, _, _)) = active_segment.as_ref() {
                                        let now = Instant::now();
                                        if now.duration_since(last_active_segment_stat_at)
                                            >= Duration::from_millis(500)
                                        {
                                            let path = path.clone();
                                            if let Ok(meta) = tokio::fs::metadata(&path).await {
                                                cached_active_segment_bytes = meta.len();
                                                has_active_segment_fs_bytes = true;
                                            }
                                            last_active_segment_stat_at = now;
                                        }

                                        let fs_total = if segment_mode {
                                            bytes_completed.saturating_add(cached_active_segment_bytes)
                                        } else {
                                            cached_active_segment_bytes
                                        };
                                        let parsed_total = if segment_mode {
                                            bytes_completed.saturating_add(progress.bytes_downloaded)
                                        } else {
                                            progress.bytes_downloaded
                                        };
                                        bytes_total = if has_active_segment_fs_bytes {
                                            fs_total
                                        } else {
                                            parsed_total
                                        };
                                    } else if segment_mode {
                                        bytes_total = bytes_completed.saturating_add(bytes_total);
                                    }

                                    total_bytes = bytes_total;
                                    total_duration = media_duration_total_secs;

                                    progress.bytes_downloaded = bytes_total;
                                    progress.duration_secs = elapsed_secs;
                                    progress.media_duration_secs = media_duration_total_secs;
                                    progress.segments_completed = segments_completed;
                                    progress.current_segment = active_segment
                                        .as_ref()
                                        .map(|(_, p, _, _)| p.to_string_lossy().to_string());

                                    progress.speed_bytes_per_sec = last_progress_snapshot
                                        .and_then(|(prev_bytes, prev_elapsed, _)| {
                                            let dt = elapsed_secs - prev_elapsed;
                                            (dt > 0.0).then_some(
                                                ((bytes_total.saturating_sub(prev_bytes)) as f64 / dt) as u64,
                                            )
                                        })
                                        .unwrap_or(0);
                                    progress.playback_ratio = last_progress_snapshot
                                        .and_then(|(_, prev_elapsed, prev_media)| {
                                            let dt = elapsed_secs - prev_elapsed;
                                            (dt > 0.0)
                                                .then_some((media_duration_total_secs - prev_media) / dt)
                                        })
                                        .unwrap_or(0.0);
                                    last_progress_snapshot =
                                        Some((bytes_total, elapsed_secs, media_duration_total_secs));

                                    observe_segment_event_send(
                                        event_tx_clone.send(SegmentEvent::Progress(progress)).await,
                                        &streamer_id_clone,
                                    );
                                }

                                // Detect mid-stream disk-full. Same pattern as
                                // ffmpeg.rs — see the matching block there
                                // for the rationale. Streamlink feeds stderr
                                // from the spawned ffmpeg process, so the
                                // same signatures apply.
                                if !disk_full_reported && is_disk_full_line(&line) {
                                    disk_full_reported = true;
                                    warn!(
                                        streamer_id = %streamer_id_clone,
                                        output_dir = %output_dir_clone.display(),
                                        "Streamlink+FFmpeg signalled disk full; emitting DiskFull event for gate"
                                    );
                                    observe_segment_event_send(
                                        event_tx_clone
                                            .send(SegmentEvent::DiskFull {
                                                output_dir: output_dir_clone.clone(),
                                                detail: format!("streamlink+ffmpeg: {}", line),
                                            })
                                            .await,
                                        &streamer_id_clone,
                                    );
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

            // Wait for the process owner before inspecting or publishing the
            // final path. Stderr EOF can race process settlement, so observing
            // EOF alone does not prove the pipeline released the file.
            let pipeline_exit = match exit_rx.await {
                Ok(pipeline_exit) => pipeline_exit,
                Err(_) => {
                    cleanup_unconfirmed = true;
                    StreamlinkPipelineExit::Failed {
                        kind: DownloadFailureKind::ProcessExit { code: None },
                        message: "Streamlink process waiter stopped without an exit result"
                            .to_string(),
                    }
                }
            };
            cleanup_unconfirmed |= event_forced_settlement.is_cancelled();

            // Do not publish a segment while the process pipeline may still be
            // writing it. The failed process settlement is reported below as
            // the terminal outcome instead.
            if !cleanup_unconfirmed
                && let Some((index, path, started_media_at, started_at)) = active_segment.take()
            {
                let size_bytes = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let duration_secs = (media_duration_total_secs - started_media_at).max(0.0);
                segments_completed = segments_completed.saturating_add(1);
                bytes_completed = bytes_completed.saturating_add(size_bytes);
                total_bytes = bytes_completed;
                if segment_mode {
                    media_duration_offset_secs += duration_secs;
                    total_duration = media_duration_offset_secs;
                } else {
                    total_duration = media_duration_total_secs;
                }
                observe_segment_event_send(
                    event_tx_clone
                        .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                            path,
                            duration_secs,
                            size_bytes,
                            index,
                            started_at: Some(started_at),
                            completed_at: Utc::now(),
                            split_reason_code: None,
                            split_reason_details_json: None,
                        }))
                        .await,
                    &streamer_id_clone,
                );
            }

            match pipeline_exit {
                StreamlinkPipelineExit::Ffmpeg(Some(0)) => {
                    // Exit code 0 — same caveat as ffmpeg: the subprocess
                    // exited cleanly but that doesn't prove the upstream
                    // stream is over. SessionLifecycle treats
                    // SubprocessExitZero as ambiguous → hysteresis.
                    observe_segment_event_send(
                        event_tx_clone
                            .send(SegmentEvent::DownloadCompleted {
                                total_bytes,
                                total_duration_secs: total_duration,
                                total_segments: segments_completed,
                                engine_signal:
                                    crate::downloader::EngineEndSignal::SubprocessExitZero,
                            })
                            .await,
                        &streamer_id_clone,
                    );
                }
                StreamlinkPipelineExit::Ffmpeg(Some(code)) => {
                    // Fallback DiskFull emission for exit code 228 if we
                    // didn't already catch it from stderr. Mirrors ffmpeg.rs.
                    if code == 228 && !disk_full_reported {
                        // The stderr loop has already exited by this point,
                        // so we don't need to update `disk_full_reported` —
                        // the variable will not be read again.
                        warn!(
                            streamer_id = %streamer_id_clone,
                            output_dir = %output_dir_clone.display(),
                            "Streamlink+FFmpeg exited with code 228; assuming disk-full"
                        );
                        observe_segment_event_send(
                            event_tx_clone
                                .send(SegmentEvent::DiskFull {
                                    output_dir: output_dir_clone.clone(),
                                    detail: "streamlink+ffmpeg exit 228 (I/O error, likely ENOSPC)"
                                        .to_string(),
                                })
                                .await,
                            &streamer_id_clone,
                        );
                    }

                    // Non-zero exit code - failure
                    observe_segment_event_send(
                        event_tx_clone
                            .send(SegmentEvent::DownloadFailed {
                                kind: DownloadFailureKind::ProcessExit { code: Some(code) },
                                message: format!("Streamlink/FFmpeg exited with code {}", code),
                            })
                            .await,
                        &streamer_id_clone,
                    );
                }
                StreamlinkPipelineExit::Ffmpeg(None) => {
                    observe_segment_event_send(
                        event_tx_clone
                            .send(SegmentEvent::DownloadFailed {
                                kind: DownloadFailureKind::ProcessExit { code: None },
                                message: "Streamlink/FFmpeg exited without an exit code"
                                    .to_string(),
                            })
                            .await,
                        &streamer_id_clone,
                    );
                }
                StreamlinkPipelineExit::Failed { kind, message } => {
                    observe_segment_event_send(
                        event_tx_clone
                            .send(SegmentEvent::DownloadFailed { kind, message })
                            .await,
                        &streamer_id_clone,
                    );
                }
            }
        }));

        let process_result = process_task.await;
        let cleanup_confirmed = process_result
            .as_ref()
            .map(|(confirmed, _)| *confirmed)
            .unwrap_or(false);
        if !cleanup_confirmed {
            forced_settlement.cancel();
        }

        let mut pipe_task = pipe_task;
        let mut streamlink_stderr_task = streamlink_stderr_task;
        let mut event_task = event_task;
        let (auxiliary_results, settlement_timed_out) = if cleanup_confirmed {
            (
                tokio::join!(pipe_task, streamlink_stderr_task, event_task),
                false,
            )
        } else {
            match tokio::time::timeout(TASK_SETTLEMENT_TIMEOUT, async {
                tokio::join!(&mut pipe_task, &mut streamlink_stderr_task, &mut event_task,)
            })
            .await
            {
                Ok(results) => (results, false),
                Err(_) => {
                    pipe_task.abort();
                    streamlink_stderr_task.abort();
                    event_task.abort();
                    (
                        tokio::join!(pipe_task, streamlink_stderr_task, event_task),
                        true,
                    )
                }
            }
        };

        let mut task_errors = Vec::new();
        match process_result {
            Ok((true, _)) => {}
            Ok((false, cleanup_error)) => task_errors.push(format!(
                "process cleanup was not confirmed: {}",
                cleanup_error.unwrap_or_else(|| "unknown cleanup failure".to_string())
            )),
            Err(error) => task_errors.push(format!("process waiter task failed: {error}")),
        }
        if settlement_timed_out {
            task_errors.push(format!(
                "auxiliary tasks did not settle within {}s after unconfirmed process cleanup",
                TASK_SETTLEMENT_TIMEOUT.as_secs()
            ));
        }
        for (task, result) in [
            ("stdout pipe", auxiliary_results.0),
            ("stderr monitor", auxiliary_results.1),
            ("event reader", auxiliary_results.2),
        ] {
            if let Err(error) = result
                && !(settlement_timed_out && error.is_cancelled())
            {
                task_errors.push(format!("{task} task failed: {error}"));
            }
        }
        if !task_errors.is_empty() {
            return Err(EngineStartError::new(
                DownloadFailureKind::Other,
                format!(
                    "Streamlink task settlement failed: {}",
                    task_errors.join("; ")
                ),
            ));
        }

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

    #[test]
    fn test_build_http_cookie_args_splits_cookie_string() {
        let args = build_http_cookie_args("a=1; b=2;  c=3");
        assert_eq!(
            args,
            vec![
                "--http-cookie".to_string(),
                "a=1".to_string(),
                "--http-cookie".to_string(),
                "b=2".to_string(),
                "--http-cookie".to_string(),
                "c=3".to_string(),
            ]
        );
    }

    #[test]
    fn cleanup_error_is_appended_without_losing_primary_failure() {
        let outcome = StreamlinkPipelineExit::Failed {
            kind: DownloadFailureKind::Network,
            message: "upstream read failed".to_string(),
        }
        .with_cleanup_error("timed out reaping ffmpeg".to_string());

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::Network);
        assert!(message.contains("upstream read failed"));
        assert!(message.contains("timed out reaping ffmpeg"));
    }

    #[test]
    fn cleanup_error_turns_successful_ffmpeg_exit_into_failure() {
        let outcome = StreamlinkPipelineExit::Ffmpeg(Some(0))
            .with_cleanup_error("failed to reap streamlink".to_string());

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::ProcessExit { code: None });
        assert!(message.contains("failed to reap streamlink"));
    }
}
