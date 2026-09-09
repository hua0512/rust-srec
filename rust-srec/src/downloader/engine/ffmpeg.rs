//! FFmpeg download engine implementation.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use pipeline_common::expand_filename_template;
use tokio::process::Child;
use tokio::time::{Duration, Instant};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, EngineStartError,
    EngineType,
};
use super::utils::{
    FfmpegEvents, FfmpegSource, PROCESS_CLEANUP_TIMEOUT, RecordingExit, redact_process_args,
    settle_engine_tasks, terminate_and_reap,
};
use crate::database::models::engine::FfmpegEngineConfig;

enum FfmpegProcessExit {
    Status(Option<i32>),
    Failed(String),
}

impl From<FfmpegProcessExit> for RecordingExit {
    fn from(exit: FfmpegProcessExit) -> Self {
        match exit {
            FfmpegProcessExit::Status(code) => Self::Status(code),
            FfmpegProcessExit::Failed(message) => Self::Failed {
                kind: DownloadFailureKind::ProcessExit { code: None },
                message,
            },
        }
    }
}

async fn settle_after_wait_failure(
    child: &mut Child,
    message: String,
) -> (FfmpegProcessExit, bool) {
    match terminate_and_reap(child, "ffmpeg", PROCESS_CLEANUP_TIMEOUT).await {
        Ok(_) => (FfmpegProcessExit::Failed(message), true),
        Err(cleanup_error) => (
            FfmpegProcessExit::Failed(format!("{message}; cleanup error: {cleanup_error}")),
            false,
        ),
    }
}

fn is_mp4_family(format: &str) -> bool {
    matches!(
        format
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "mp4" | "mov" | "m4v"
    )
}

fn contains_option(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| {
        arg == name
            || arg
                .strip_prefix(name)
                .is_some_and(|suffix| suffix.starts_with('='))
    })
}

fn append_faststart_option(
    args: &mut Vec<String>,
    output_args: &[String],
    output_format: &str,
    segment_mode: bool,
) {
    if !is_mp4_family(output_format) {
        return;
    }

    let (option, value) = if segment_mode {
        ("-segment_format_options", "movflags=+faststart")
    } else {
        ("-movflags", "+faststart")
    };

    if !contains_option(output_args, option) {
        args.extend([option.to_string(), value.to_string()]);
    }
}

/// FFmpeg-based download engine.
pub struct FfmpegEngine {
    /// Engine configuration.
    config: FfmpegEngineConfig,
    /// Cached version string.
    version: Option<String>,
    #[cfg(test)]
    fixture: Option<super::utils::test_support::RecordingFixture>,
}

impl FfmpegEngine {
    /// Create a new FFmpeg engine with default configuration.
    pub fn new() -> Self {
        Self::with_config(FfmpegEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: FfmpegEngineConfig) -> Self {
        let version = super::utils::probe_version_sync(&config.binary_path, "-version")
            .and_then(|output| output.lines().next().map(str::to_owned));
        Self::with_version(config, version)
    }

    /// Probe a configured executable without blocking the async runtime.
    pub async fn with_config_async(config: FfmpegEngineConfig) -> Self {
        let version = super::utils::probe_version(&config.binary_path, "-version")
            .await
            .and_then(|output| output.lines().next().map(str::to_owned));
        Self::with_version(config, version)
    }

    fn with_version(config: FfmpegEngineConfig, version: Option<String>) -> Self {
        Self {
            config,
            version,
            #[cfg(test)]
            fixture: None,
        }
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

        // 8. File size limit if configured
        // After that size download will be stopped
        if config.max_segment_size_bytes > 0 {
            args.extend(["-fs".to_string(), config.max_segment_size_bytes.to_string()]);
        }

        let segment_mode = config.max_segment_duration_secs > 0;

        // Segment options if splitting is enabled
        if segment_mode {
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

        append_faststart_option(
            &mut args,
            &self.config.output_args,
            &config.output_format,
            segment_mode,
        );

        // Segment-open messages and progress records are an engine contract.
        // FFmpeg uses the final global options, overriding user -v/-loglevel/-nostats.
        args.extend([
            "-loglevel".to_string(),
            "info".to_string(),
            "-stats".to_string(),
        ]);

        // Output path
        let output_directory = if segment_mode {
            PathBuf::from(config.output_dir.to_string_lossy().replace('%', "%%"))
        } else {
            config.output_dir.clone()
        };
        let output_path = output_directory.join(format!(
            "{}.{}",
            config.filename_template, config.output_format
        ));

        if segment_mode {
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

    async fn run(&self, handle: Arc<DownloadHandle>) -> std::result::Result<(), EngineStartError> {
        let config = handle.config_snapshot();
        // `DownloadManager::prepare_output_dir` runs before engine startup,
        // enforcing the output-root write gate and classifying directory errors.
        let args = self.build_args(&config);
        let segment_mode = config.max_segment_duration_secs > 0;
        let single_output_path = if segment_mode {
            None
        } else {
            args.last().map(|s| PathBuf::from(s.clone()))
        };

        info!(
            "Starting ffmpeg download for streamer {} with args: {:?}",
            config.streamer_id,
            redact_process_args(&args)
        );

        // Spawn ffmpeg process
        let mut command = process_utils::tokio_command(&self.config.binary_path);
        command.args(&args);
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            command = fixture.command(false);
        }
        crate::utils::configure_ffmpeg_locale(&mut command);
        command
            .stdin(Stdio::piped()) // allow graceful stop via 'q'
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            EngineStartError::new(
                DownloadFailureKind::Configuration,
                format!("Failed to spawn ffmpeg: {}", e),
            )
        })?;

        let mut stdin = child.stdin.take();
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let mut message = "Failed to capture ffmpeg stderr".to_string();
                if let Err(cleanup_error) =
                    terminate_and_reap(&mut child, "ffmpeg", PROCESS_CLEANUP_TIMEOUT).await
                {
                    message.push_str("; cleanup error: ");
                    message.push_str(&cleanup_error);
                }
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };

        // 2. Wait for exit (supports graceful stop on cancellation)
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<FfmpegProcessExit>();
        let cancellation_token = handle.cancellation_token.clone();
        let forced_settlement = CancellationToken::new();
        let process_forced_settlement = forced_settlement.clone();
        let started_instant = Instant::now();
        let graceful_stop_timeout_secs = self.config.graceful_stop_timeout_secs;
        let budget_handle = handle.clone();
        #[cfg(test)]
        let inject_unconfirmed_cleanup = self
            .fixture
            .as_ref()
            .is_some_and(|fixture| fixture.unconfirmed);
        #[cfg(test)]
        let wait_fixture = self.fixture.clone();
        let process_task = AbortOnDropHandle::new(tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;

            // Resolved per use rather than once here: `set_stop_deadline` runs
            // when the stop is requested, which is after this task starts, so a
            // value captured now would still be the unclamped configured one.
            let configured_graceful_stop = Duration::from_secs(graceful_stop_timeout_secs as u64);
            let graceful_stop_timeout =
                || budget_handle.graceful_stop_budget(configured_graceful_stop);

            let (process_exit, cleanup_confirmed) = tokio::select! {
                status = async {
                    #[cfg(test)]
                    if let Some(fixture) = &wait_fixture { fixture.inject_wait_failure().await?; }
                    child.wait().await
                } => {
                    match status {
                        Ok(exit_status) => (FfmpegProcessExit::Status(exit_status.code()), true),
                        Err(error) => {
                            let message = format!("Failed to wait for ffmpeg: {error}");
                            error!(%error, "Error waiting for ffmpeg process");
                            settle_after_wait_failure(
                                &mut child,
                                message,
                            ).await
                        }
                    }
                }
                _ = cancellation_token.cancelled() => {
                    debug!("FFmpeg stop requested, sending 'q' for graceful exit");
                    if let Some(mut stdin) = stdin.take() {
                        if let Err(e) = stdin.write_all(b"q").await {
                            warn!(error = %e, "Failed to send graceful stop to ffmpeg");
                        }
                        if let Err(e) = stdin.flush().await {
                            warn!(error = %e, "Failed to flush ffmpeg stop command");
                        }
                        if let Err(e) = stdin.shutdown().await {
                            warn!(error = %e, "Failed to close ffmpeg stdin");
                        }
                    }

                    match tokio::time::timeout(graceful_stop_timeout(), child.wait()).await {
                        Ok(Ok(exit_status)) => {
                            (FfmpegProcessExit::Status(exit_status.code()), true)
                        }
                        Ok(Err(error)) => {
                            let message =
                                format!("Failed to wait for ffmpeg after stop request: {error}");
                            error!(%error, "Error waiting for ffmpeg after stop request");
                            settle_after_wait_failure(
                                &mut child,
                                message,
                            ).await
                        }
                        Err(_) => {
                            warn!("FFmpeg did not exit in time; killing process");
                            match terminate_and_reap(
                                &mut child,
                                "ffmpeg",
                                PROCESS_CLEANUP_TIMEOUT,
                            ).await {
                                Ok(exit_code) => (FfmpegProcessExit::Status(exit_code), true),
                                Err(cleanup_error) => {
                                    error!(%cleanup_error, "Failed to stop ffmpeg process");
                                    (
                                        FfmpegProcessExit::Failed(format!(
                                            "FFmpeg did not exit within the graceful stop timeout; cleanup error: {cleanup_error}"
                                        )),
                                        false,
                                    )
                                }
                            }
                        }
                    }
                }
            };

            // Tests inject a failed cleanup report after safely reaping their
            // real helper child, avoiding an intentionally leaked process.
            #[cfg(test)]
            let (process_exit, cleanup_confirmed) = if inject_unconfirmed_cleanup {
                (
                    FfmpegProcessExit::Failed("injected cleanup failure".to_owned()),
                    false,
                )
            } else {
                (process_exit, cleanup_confirmed)
            };
            if !cleanup_confirmed {
                process_forced_settlement.cancel();
            }
            let cleanup_error = if cleanup_confirmed {
                None
            } else {
                match &process_exit {
                    FfmpegProcessExit::Failed(message) => Some(message.clone()),
                    FfmpegProcessExit::Status(_) => {
                        Some("ffmpeg process cleanup was not confirmed".to_string())
                    }
                }
            };
            if exit_tx.send(process_exit).is_err() {
                debug!("Download exit receiver dropped before ffmpeg completed");
            }
            (cleanup_confirmed, cleanup_error)
        }));

        let events = FfmpegEvents {
            source: FfmpegSource::Direct,
            segment_mode,
            single_output_path,
            started_instant,
            streamer_id: config.streamer_id.clone(),
            output_dir: config.output_dir.clone(),
            event_tx: handle.event_tx.clone(),
            forced_settlement: forced_settlement.clone(),
        };
        let event_task = AbortOnDropHandle::new(tokio::spawn(events.run(stderr, exit_rx)));
        settle_engine_tasks(
            process_task,
            forced_settlement,
            FfmpegSource::Direct,
            vec![("event reader", event_task)],
        )
        .await
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

    #[cfg(unix)]
    #[tokio::test]
    async fn output_error_precedes_terminal_even_when_ffmpeg_exits_zero() {
        use super::super::utils::test_support::{assert_output_failure, script};
        let dir = tempfile::tempdir().unwrap();
        let binary = script(
            dir.path(),
            "ffmpeg",
            "printf '%s\\n' 'Error opening output file: Permission denied' >&2\nexit 0\n",
        );
        let engine = FfmpegEngine {
            config: FfmpegEngineConfig {
                binary_path: binary,
                ..Default::default()
            },
            version: None,
            fixture: None,
        };
        assert_output_failure(&engine, dir.path()).await;
    }

    fn engine_with_output_args(output_args: &[&str]) -> FfmpegEngine {
        FfmpegEngine {
            config: FfmpegEngineConfig {
                output_args: output_args.iter().map(ToString::to_string).collect(),
                ..Default::default()
            },
            version: None,
            fixture: None,
        }
    }

    #[test]
    fn event_logging_overrides_quiet_user_options_and_escapes_literal_directories() {
        let engine = FfmpegEngine {
            config: FfmpegEngineConfig {
                input_args: vec!["-v".to_owned(), "quiet".to_owned()],
                output_args: vec![
                    "-loglevel".to_owned(),
                    "warning".to_owned(),
                    "-nostats".to_owned(),
                ],
                ..Default::default()
            },
            version: None,
            fixture: None,
        };
        for segment_duration in [0, 10] {
            let mut config = download_config("ts", segment_duration);
            config.output_dir = PathBuf::from("root/50%d");
            config.filename_template =
                crate::utils::filename::sanitize_filename_for_template("name%Y%n");
            let args = engine.build_args(&config);
            assert_eq!(
                &args[args.len() - 4..args.len() - 1],
                ["-loglevel", "info", "-stats"]
            );
            let path = args.last().unwrap();
            if segment_duration == 0 {
                assert_eq!(path, "root/50%d/name%Y%n.ts");
            } else {
                assert_eq!(path, "root/50%%d/name%%Y%%n.ts");
                assert_eq!(
                    pipeline_common::expand_path_template(path),
                    "root/50%d/name%Y%n.ts"
                );
            }
        }
    }

    fn download_config(format: &str, segment_duration_secs: u64) -> DownloadConfig {
        DownloadConfig::new(
            "https://example.com/live",
            "recordings",
            "streamer-id",
            "Streamer",
            "session-id",
        )
        .with_filename_template("recording-%Y%m%d-%H%M%S")
        .with_output_format(format)
        .with_max_segment_duration(segment_duration_secs)
    }

    #[tokio::test]
    async fn recording_stop_settlement() {
        use super::super::utils::test_support::{
            RecordingFixture, StopCase, assert_recording_stop,
        };
        for case in StopCase::ALL {
            let dir = tempfile::tempdir().unwrap();
            let fixture = RecordingFixture::new(dir.path(), case);
            let engine = FfmpegEngine {
                config: FfmpegEngineConfig::default(),
                version: None,
                fixture: Some(fixture.clone()),
            };
            assert_recording_stop(engine, fixture, case).await;
        }
    }

    #[tokio::test]
    async fn recording_segment_contracts() {
        use super::super::utils::recording_contracts::{ContractCase, assert_recording_contract};
        use super::super::utils::test_support::RecordingFixture;
        for case in ContractCase::ALL {
            let directory = tempfile::tempdir().unwrap();
            let engine = FfmpegEngine {
                config: FfmpegEngineConfig::default(),
                version: None,
                fixture: Some(RecordingFixture::contract(directory.path(), case, false)),
            };
            assert_recording_contract(engine, directory.path(), case).await;
        }
    }

    fn has_arg_pair(args: &[String], option: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == option && pair[1] == value)
    }

    #[test]
    fn mp4_output_uses_faststart() {
        let args = engine_with_output_args(&[]).build_args(&download_config("mp4", 0));

        assert!(has_arg_pair(&args, "-movflags", "+faststart"));
        assert!(!contains_option(&args, "-segment_format_options"));
    }

    #[test]
    fn segmented_mp4_output_passes_faststart_to_the_segment_muxer() {
        let args = engine_with_output_args(&[]).build_args(&download_config("MP4", 60));

        assert!(has_arg_pair(
            &args,
            "-segment_format_options",
            "movflags=+faststart"
        ));
        assert!(!contains_option(&args, "-movflags"));
    }

    #[test]
    fn non_mp4_output_does_not_use_faststart() {
        let args = engine_with_output_args(&[]).build_args(&download_config("mkv", 0));

        assert!(!contains_option(&args, "-movflags"));
        assert!(!contains_option(&args, "-segment_format_options"));
    }

    #[test]
    fn custom_faststart_option_is_not_overridden() {
        let args = engine_with_output_args(&["-movflags=+frag_keyframe"])
            .build_args(&download_config("mp4", 0));

        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-movflags"))
                .count(),
            1
        );
        assert!(!args.iter().any(|arg| arg == "+faststart"));
    }

    #[test]
    fn test_engine_type() {
        let engine = FfmpegEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Ffmpeg);
    }
}
