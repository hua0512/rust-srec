//! Streamlink download engine implementation.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use pipeline_common::expand_filename_template;
use process_utils::ContainedChild;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Duration, Instant};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use tracing::{debug, error, info, warn};

use super::traits::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, EngineStartError,
    EngineType,
};
use super::utils::{
    FfmpegEvents, FfmpegSource, PROCESS_CLEANUP_TIMEOUT, RecordingExit, redact_process_args,
    settle_engine_tasks,
};
use crate::database::models::engine::StreamlinkEngineConfig;

/// Cap for Streamlink's cooperative or natural exit before forcing containment.
/// Used after FFmpeg exits and during cancellation, where the remaining shared
/// stop budget can shorten this grace further.
const STREAMLINK_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

mod control;

#[cfg(test)]
mod shutdown_tests;

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
    #[cfg(test)]
    fixture: Option<super::utils::test_support::RecordingFixture>,
    #[cfg(test)]
    shutdown_fixture: Option<shutdown_tests::Fixture>,
}

impl StreamlinkEngine {
    /// Create a new Streamlink engine with default configuration.
    pub fn new() -> Self {
        Self::with_config(StreamlinkEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: StreamlinkEngineConfig) -> Self {
        let version = super::utils::probe_version_sync(&config.binary_path, "--version")
            .map(|output| output.trim().to_owned());
        Self::with_version(config, version)
    }

    /// Probe a configured executable without blocking the async runtime.
    pub async fn with_config_async(config: StreamlinkEngineConfig) -> Self {
        let version = super::utils::probe_version(&config.binary_path, "--version")
            .await
            .map(|output| output.trim().to_owned());
        Self::with_version(config, version)
    }

    fn with_version(config: StreamlinkEngineConfig, version: Option<String>) -> Self {
        let ffmpeg_path = config
            .ffmpeg_path
            .clone()
            .or_else(|| std::env::var("FFMPEG_PATH").ok())
            .unwrap_or_else(|| "ffmpeg".to_string());

        Self {
            config,
            ffmpeg_path,
            version,
            #[cfg(test)]
            fixture: None,
            #[cfg(test)]
            shutdown_fixture: None,
        }
    }

    fn ffmpeg_command(&self, args: &[String]) -> tokio::process::Command {
        let mut command = process_utils::tokio_command(&self.ffmpeg_path);
        command.args(args);
        command
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
                // Pass an unparsed cookie value to Streamlink for validation.
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

        // The stderr reader requires info-level segment announcements and stats.
        args.extend([
            "-loglevel".to_string(),
            "info".to_string(),
            "-stats".to_string(),
        ]);

        // The directory has already been expanded; only the basename is a template.
        let output_directory = if config.max_segment_duration_secs > 0 {
            PathBuf::from(config.output_dir.to_string_lossy().replace('%', "%%"))
        } else {
            config.output_dir.clone()
        };
        let output_path = output_directory.join(format!(
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

impl From<StreamlinkPipelineExit> for RecordingExit {
    fn from(exit: StreamlinkPipelineExit) -> Self {
        match exit {
            StreamlinkPipelineExit::Ffmpeg(code) => Self::Status(code),
            StreamlinkPipelineExit::Failed { kind, message } => Self::Failed { kind, message },
        }
    }
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

/// How the streamlink process settled once FFmpeg had already exited with a
/// zero status. Produced by `settle_streamlink_after_ffmpeg_exit`.
#[derive(Debug)]
enum StreamlinkSettlement {
    /// Streamlink exited with a zero status inside the grace window.
    ExitedCleanly,
    /// Streamlink exited with a non-zero status inside the grace window.
    ExitedWithFailure { code: Option<i32>, status: String },
    /// `Child::wait` on streamlink returned an error.
    WaitFailed(String),
    /// Streamlink was still running when the grace window elapsed.
    StillRunning,
}

/// Lets a stdout pipe failure recorded by the pipe task take precedence over
/// `outcome`.
///
/// The pipe task stores into its `pipe_failure` slot when it cannot read
/// streamlink's stdout or write FFmpeg's stdin, which truncates the recording
/// no matter how the two processes exited; `outcome` is folded in as the
/// secondary error, and a clean `Ffmpeg(Some(0))` contributes none because
/// `failure_summary` returns `None` for it.
fn apply_pipe_failure(
    outcome: StreamlinkPipelineExit,
    pipe_failure: Option<String>,
) -> StreamlinkPipelineExit {
    let Some(message) = pipe_failure else {
        return outcome;
    };
    let mut failure = StreamlinkPipelineExit::Failed {
        kind: DownloadFailureKind::Network,
        message,
    };
    if let Some(secondary_error) = outcome.failure_summary() {
        failure = failure.with_secondary_error(secondary_error);
    }
    failure
}

/// Maps `settlement` onto the pipeline outcome for a zero-status FFmpeg exit.
///
/// Streamlink closes its stdout before its own process exits, so the pipe task
/// sees EOF and drops FFmpeg's stdin while the streamlink process is still
/// tearing down: a zero FFmpeg status is a clean pipeline whenever streamlink
/// also exits cleanly, and only a failure otherwise. Precedence and wording
/// follow the `streamlink.wait()` select branch — a streamlink failure outranks
/// a recorded pipe failure, which outranks the clean FFmpeg exit — so both
/// orderings report the same thing for the same streamlink status.
fn clean_ffmpeg_exit_outcome(
    settlement: &StreamlinkSettlement,
    pipe_failure: Option<String>,
) -> StreamlinkPipelineExit {
    match settlement {
        StreamlinkSettlement::ExitedCleanly => {
            apply_pipe_failure(StreamlinkPipelineExit::Ffmpeg(Some(0)), pipe_failure)
        }
        StreamlinkSettlement::ExitedWithFailure { code, status } => {
            StreamlinkPipelineExit::Failed {
                kind: DownloadFailureKind::ProcessExit { code: *code },
                message: format!("Streamlink exited with status {status}"),
            }
        }
        StreamlinkSettlement::WaitFailed(error) => StreamlinkPipelineExit::Failed {
            kind: DownloadFailureKind::ProcessExit { code: None },
            message: format!("Failed to wait for Streamlink: {error}"),
        },
        StreamlinkSettlement::StillRunning => StreamlinkPipelineExit::Failed {
            kind: DownloadFailureKind::Other,
            message: "FFmpeg exited before Streamlink completed".to_string(),
        },
    }
}

fn append_cleanup_result(message: &mut String, result: std::result::Result<Option<i32>, String>) {
    if let Err(cleanup_error) = result {
        message.push_str("; cleanup error: ");
        message.push_str(&cleanup_error);
    }
}

struct StopBudget {
    handle: Arc<DownloadHandle>,
    configured: Duration,
    observed: Mutex<Option<Instant>>,
}

impl StopBudget {
    fn deadline(&self) -> Instant {
        let current = Instant::now() + self.handle.graceful_stop_budget(self.configured);
        let mut observed = self.observed.lock();
        let deadline = observed.get_or_insert(current);
        *deadline = (*deadline).min(current);
        *deadline
    }

    fn peer_deadlines(&self, previous: Instant) -> (Instant, Instant) {
        let hard = self.deadline().min(previous + PROCESS_CLEANUP_TIMEOUT);
        let reserve =
            Duration::from_secs(1).min(hard.saturating_duration_since(Instant::now()) / 4);
        (previous.min(hard - reserve), hard)
    }

    fn process_deadlines(&self, source: bool) -> (Instant, Instant) {
        let hard = self.deadline();
        let remaining = hard.saturating_duration_since(Instant::now());
        let cleanup = Duration::from_secs(1).min(remaining / 4);
        let finalize = if source {
            Duration::from_secs(2).min(remaining / 2)
        } else {
            Duration::ZERO
        };
        (hard - cleanup - finalize, hard - finalize)
    }

    async fn within<T, F: std::future::Future<Output = T>>(
        &self,
        future: F,
        mut deadline: Instant,
        source: bool,
    ) -> Option<T> {
        let mut future = std::pin::pin!(future);
        let mut updates = self.handle.stop_deadline_updates();
        loop {
            drop(updates.borrow_and_update());
            deadline = deadline.min(if source {
                self.process_deadlines(true).0
            } else {
                self.deadline()
            });
            tokio::select! {
                biased;
                result = &mut future => return Some(result),
                changed = updates.changed() => {
                    if changed.is_err() { return tokio::time::timeout_at(deadline, future).await.ok(); }
                }
                _ = tokio::time::sleep_until(deadline) => return None,
            }
        }
    }
}

enum ChildWait {
    Exited(Result<std::process::ExitStatus, process_utils::ContainmentError>),
    Expired,
    Stopped(Instant),
}

async fn wait_for_peer(
    child: &mut ContainedChild,
    timeout: Duration,
    budget: &StopBudget,
) -> ChildWait {
    let deadline = Instant::now() + timeout;
    tokio::select! {
        biased;
        _ = budget.handle.cancellation_token.cancelled() => ChildWait::Stopped(deadline),
        result = tokio::time::timeout_at(deadline, child.wait()) => match result {
            Ok(result) => ChildWait::Exited(result),
            Err(_) => ChildWait::Expired,
        },
    }
}

async fn terminate_and_reap(
    child: &mut ContainedChild,
    process_name: &str,
    timeout: Duration,
    budget: &StopBudget,
) -> std::result::Result<Option<i32>, String> {
    let mut stopping = budget.handle.cancellation_token.is_cancelled();
    let mut deadline = Instant::now() + timeout;
    let mut updates = budget.handle.stop_deadline_updates();
    let result = loop {
        stopping |= updates.borrow_and_update().is_some();
        if stopping {
            deadline = deadline.min(budget.deadline());
        }
        tokio::select! {
            biased;
            _ = budget.handle.cancellation_token.cancelled(), if !stopping => stopping = true,
            changed = updates.changed() => {
                if changed.is_err() { break child.terminate_tree_until(deadline).await; }
            }
            result = child.terminate_tree_until(deadline) => break result,
        }
    };
    result
        .map(|status| status.code())
        .map_err(|error| format!("failed to contain and reap {process_name}: {error}"))
}

async fn wait_then_terminate(
    child: &mut ContainedChild,
    process_name: &str,
    timeout: Duration,
    budget: &StopBudget,
) -> (std::result::Result<Option<i32>, String>, bool) {
    let error = match wait_for_peer(child, timeout, budget).await {
        ChildWait::Exited(Ok(status)) => return (Ok(status.code()), true),
        ChildWait::Stopped(previous) => {
            let (wait, hard) = budget.peer_deadlines(previous);
            let (result, contained, natural) =
                wait_for_stop(child, process_name, wait, hard, budget, false).await;
            return (
                if natural {
                    result
                } else {
                    result.and_then(|_| Err(format!("{process_name} exceeded the stop deadline")))
                },
                contained,
            );
        }
        ChildWait::Exited(Err(error)) => format!("failed to wait for {process_name}: {error}"),
        ChildWait::Expired => format!("{process_name} did not exit within the settlement timeout"),
    };
    match terminate_and_reap(child, process_name, PROCESS_CLEANUP_TIMEOUT, budget).await {
        Ok(_) => (Err(error), true),
        Err(cleanup) => (Err(format!("{error}; cleanup error: {cleanup}")), false),
    }
}

/// Stop-phase waits and containment share the attempt's absolute deadline.
/// A forced zero exit is still not proof of a natural buffered drain.
async fn wait_for_stop(
    child: &mut ContainedChild,
    process_name: &str,
    mut wait_deadline: Instant,
    mut hard_deadline: Instant,
    budget: &StopBudget,
    source: bool,
) -> (std::result::Result<Option<i32>, String>, bool, bool) {
    let mut updates = budget.handle.stop_deadline_updates();
    let waited = loop {
        drop(updates.borrow_and_update());
        let (wait, hard) = budget.process_deadlines(source);
        wait_deadline = wait_deadline.min(wait);
        hard_deadline = hard_deadline.min(hard);
        tokio::select! {
            changed = updates.changed() => {
                if changed.is_err() { break tokio::time::timeout_at(wait_deadline, child.wait()).await; }
            }
            result = tokio::time::timeout_at(wait_deadline, child.wait()) => break result,
        }
    };
    let wait_error = match waited {
        Ok(Ok(status)) => return (Ok(status.code()), true, true),
        Ok(Err(error)) => Some(format!("failed to wait for {process_name}: {error}")),
        Err(_) => None,
    };
    warn!(
        process = process_name,
        "Recording process did not finish within its stop deadline; forcing containment"
    );
    let contained = loop {
        drop(updates.borrow_and_update());
        hard_deadline = hard_deadline.min(budget.process_deadlines(source).1);
        tokio::select! {
            changed = updates.changed() => {
                if changed.is_err() { break child.terminate_tree_until(hard_deadline).await; }
            }
            result = child.terminate_tree_until(hard_deadline) => break result,
        }
    };
    match contained {
        Ok(status) => (wait_error.map_or(Ok(status.code()), Err), true, false),
        Err(error) => (
            Err(format!(
                "failed to contain {process_name} before the stop deadline: {error}"
            )),
            false,
            false,
        ),
    }
}

/// Waits up to `timeout` for streamlink to exit on its own after FFmpeg has
/// already exited, terminating it through `terminate_and_reap` when it does not.
///
/// The second element is `Ok(())` whenever streamlink is known to be reaped, so
/// the caller can keep the `cleanup_confirmed` flag that drives
/// `forced_settlement` accurate; its `Err` carries a cleanup error for
/// `StreamlinkPipelineExit::with_cleanup_error`.
async fn settle_streamlink_after_ffmpeg_exit(
    streamlink: &mut ContainedChild,
    timeout: Duration,
    budget: &StopBudget,
) -> (StreamlinkSettlement, std::result::Result<(), String>) {
    let settlement = match wait_for_peer(streamlink, timeout, budget).await {
        ChildWait::Exited(Ok(status)) if status.success() => {
            return (StreamlinkSettlement::ExitedCleanly, Ok(()));
        }
        ChildWait::Exited(Ok(status)) => {
            return (
                StreamlinkSettlement::ExitedWithFailure {
                    code: status.code(),
                    status: status.to_string(),
                },
                Ok(()),
            );
        }
        ChildWait::Stopped(previous) => {
            let (wait, hard) = budget.peer_deadlines(previous);
            let (result, contained, natural) =
                wait_for_stop(streamlink, "streamlink", wait, hard, budget, false).await;
            let settlement = match &result {
                Ok(Some(0)) if natural => StreamlinkSettlement::ExitedCleanly,
                Ok(code) if natural => StreamlinkSettlement::ExitedWithFailure {
                    code: *code,
                    status: format!("exit code {code:?}"),
                },
                _ => StreamlinkSettlement::StillRunning,
            };
            return (
                settlement,
                if contained {
                    Ok(())
                } else {
                    result.map(|_| ())
                },
            );
        }
        ChildWait::Exited(Err(error)) => StreamlinkSettlement::WaitFailed(error.to_string()),
        ChildWait::Expired => StreamlinkSettlement::StillRunning,
    };
    (
        settlement,
        terminate_and_reap(streamlink, "streamlink", PROCESS_CLEANUP_TIMEOUT, budget)
            .await
            .map(|_| ()),
    )
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
        let stop_budget = Arc::new(StopBudget {
            handle: handle.clone(),
            configured: Duration::from_secs(self.config.graceful_stop_timeout_secs as u64),
            observed: Mutex::new(None),
        });
        // `DownloadManager::prepare_output_dir` runs before engine startup,
        // enforcing the output-root write gate and classifying directory errors.
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
            config.streamer_id,
            redact_process_args(&streamlink_args)
        );

        // Spawn streamlink process
        let mut streamlink_command = process_utils::tokio_command(&self.config.binary_path);
        #[cfg(not(test))]
        let fixture_control = false;
        #[cfg(test)]
        let fixture_control = self.shutdown_fixture.is_some();
        let profile_available = if control::Companion::supports_version(self.version.as_deref()) {
            if fixture_control {
                true
            } else {
                tokio::select! {
                    supported = control::Companion::cached_capability(&self.config.binary_path) => supported,
                    _ = handle.cancellation_token.cancelled() => {
                        return Err(EngineStartError::new(DownloadFailureKind::Other, "Streamlink startup was cancelled"));
                    }
                }
            }
        } else {
            false
        };
        let companion = if profile_available {
            match control::Companion::prepare().await {
                Ok(companion) => {
                    companion.configure(&mut streamlink_command);
                    Some(companion)
                }
                Err(error) => {
                    warn!(%error, "Streamlink control unavailable; cooperative draining cannot be verified");
                    None
                }
            }
        } else {
            None
        };
        streamlink_command.args(&streamlink_args);
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            streamlink_command = fixture.command(true);
        }
        #[cfg(test)]
        if let Some(fixture) = &self.shutdown_fixture {
            streamlink_command = fixture.command(true);
            if let Some(companion) = &companion {
                companion.configure_environment(&mut streamlink_command);
            }
        }
        streamlink_command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut streamlink = ContainedChild::spawn(&mut streamlink_command).map_err(|e| {
            EngineStartError::new(
                DownloadFailureKind::Configuration,
                format!("Failed to spawn streamlink: {}", e),
            )
        })?;

        let mut streamlink_stdout = match streamlink.take_stdout() {
            Some(stdout) => stdout,
            None => {
                let mut message = "Failed to capture streamlink stdout".to_string();
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget,
                    )
                    .await,
                );
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };
        let streamlink_stderr = match streamlink.take_stderr() {
            Some(stderr) => stderr,
            None => {
                let mut message = "Failed to capture streamlink stderr".to_string();
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget,
                    )
                    .await,
                );
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };

        // Spawn ffmpeg process with stdin piped
        let mut ffmpeg_command = self.ffmpeg_command(&ffmpeg_args);
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            ffmpeg_command = fixture.command(false);
        }
        #[cfg(test)]
        if let Some(fixture) = &self.shutdown_fixture {
            ffmpeg_command = fixture.command(false);
        }
        crate::utils::configure_ffmpeg_locale(&mut ffmpeg_command);
        ffmpeg_command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut ffmpeg = match ContainedChild::spawn(&mut ffmpeg_command) {
            Ok(ffmpeg) => ffmpeg,
            Err(error) => {
                let mut message = format!("Failed to spawn ffmpeg: {error}");
                append_cleanup_result(
                    &mut message,
                    terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget,
                    )
                    .await,
                );
                return Err(EngineStartError::new(
                    DownloadFailureKind::Configuration,
                    message,
                ));
            }
        };

        let mut ffmpeg_stdin = match ffmpeg.take_stdin() {
            Some(stdin) => stdin,
            None => {
                let mut message = "Failed to capture ffmpeg stdin".to_string();
                let (streamlink_cleanup, ffmpeg_cleanup) = tokio::join!(
                    terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget
                    ),
                    terminate_and_reap(
                        &mut ffmpeg,
                        "ffmpeg",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget
                    ),
                );
                append_cleanup_result(&mut message, streamlink_cleanup);
                append_cleanup_result(&mut message, ffmpeg_cleanup);
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };
        let ffmpeg_stderr = match ffmpeg.take_stderr() {
            Some(stderr) => stderr,
            None => {
                let mut message = "Failed to capture ffmpeg stderr".to_string();
                let (streamlink_cleanup, ffmpeg_cleanup) = tokio::join!(
                    terminate_and_reap(
                        &mut streamlink,
                        "streamlink",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget
                    ),
                    terminate_and_reap(
                        &mut ffmpeg,
                        "ffmpeg",
                        PROCESS_CLEANUP_TIMEOUT,
                        &stop_budget
                    ),
                );
                append_cleanup_result(&mut message, streamlink_cleanup);
                append_cleanup_result(&mut message, ffmpeg_cleanup);
                return Err(EngineStartError::new(DownloadFailureKind::Other, message));
            }
        };

        let cancellation_token = handle.cancellation_token.clone();
        let started_instant = Instant::now();
        let process_stop_budget = stop_budget.clone();

        // 2. Spawn a waiter task for both processes.
        //
        // Cancellation stops the producer first. The pipe keeps forwarding emitted
        // bytes until EOF, then closes FFmpeg's stdin so its output can finalize.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<StreamlinkPipelineExit>();
        let cancellation_token_wait = cancellation_token.clone();
        let forced_settlement = CancellationToken::new();
        let process_forced_settlement = forced_settlement.clone();
        let pipe_failure = Arc::new(Mutex::new(None::<String>));
        let pipe_eof = Arc::new(AtomicBool::new(false));
        let process_pipe_eof = pipe_eof.clone();
        let process_pipe_failure = pipe_failure.clone();
        #[cfg(test)]
        let inject_unconfirmed_cleanup = self
            .fixture
            .as_ref()
            .is_some_and(|fixture| fixture.unconfirmed);
        #[cfg(test)]
        let wait_fixture = self.fixture.clone();
        #[cfg(test)]
        let shutdown_fixture = self.shutdown_fixture.clone();
        let process_task = AbortOnDropHandle::new(tokio::spawn(async move {
            // Resolved per use rather than once here: `set_stop_deadline` runs
            // when the stop is requested, which is after this task starts, so a
            // value captured now would still be the unclamped configured one.
            let budget = process_stop_budget;
            let ffmpeg_stop_timeout = || budget.handle.graceful_stop_budget(budget.configured);

            let (pipeline_exit, cleanup_confirmed) = tokio::select! {
                _ = cancellation_token_wait.cancelled() => {
                    let stop_deadline = budget.deadline();
                    let remaining = stop_deadline.saturating_duration_since(Instant::now());
                    let cleanup_reserve = Duration::from_secs(1).min(remaining / 4);
                    let finalize_reserve = Duration::from_secs(2).min(remaining / 2);
                    let streamlink_deadline = stop_deadline - cleanup_reserve - finalize_reserve;
                    let cooperative = if let Some(companion) = &companion {
                        #[cfg(test)]
                        if let Some(fixture) = &shutdown_fixture { fixture.negotiating_stop(); }
                        budget.within(companion.request_stop(streamlink_deadline), streamlink_deadline, true)
                            .await.unwrap_or(false)
                    } else { false };
                    if !cooperative { match streamlink.request_shutdown() {
                        Ok(true) => debug!("Requested cooperative Streamlink shutdown"),
                        Ok(false) => debug!("Waiting for Streamlink exit before contained termination"),
                        Err(error) => warn!(%error, "Could not request cooperative Streamlink shutdown; using bounded exit wait"),
                    } }
                    let source_deadline = if cooperative { streamlink_deadline } else {
                        streamlink_deadline.min(Instant::now() + STREAMLINK_SETTLE_TIMEOUT)
                    };
                    let (streamlink_cleanup, streamlink_confirmed, source_natural) = wait_for_stop(
                        &mut streamlink,
                        "streamlink",
                        source_deadline,
                        stop_deadline - finalize_reserve,
                        &budget,
                        true,
                    ).await;
                    let (ffmpeg_result, ffmpeg_confirmed, ffmpeg_natural) =
                        wait_for_stop(&mut ffmpeg, "ffmpeg", stop_deadline - cleanup_reserve, stop_deadline, &budget, false).await;
                    let mut outcome = match ffmpeg_result {
                        Ok(code) => StreamlinkPipelineExit::Ffmpeg(code),
                        Err(message) => StreamlinkPipelineExit::Failed {
                            kind: DownloadFailureKind::ProcessExit { code: None },
                            message,
                        },
                    };
                    let source_clean = matches!(streamlink_cleanup, Ok(Some(0))) && source_natural;
                    if let Err(cleanup_error) = streamlink_cleanup {
                            warn!(%cleanup_error, "Failed to stop streamlink cleanly");
                            outcome = outcome.with_cleanup_error(cleanup_error);
                    }
                    let drain = if cooperative && source_clean && ffmpeg_natural
                        && matches!(outcome, StreamlinkPipelineExit::Ffmpeg(Some(0)))
                        && process_pipe_eof.load(Ordering::Acquire) {
                        match &companion {
                            Some(companion) => budget.within(companion.drain_result(stop_deadline), stop_deadline, false)
                                .await.unwrap_or_else(|| Err("companion verification exceeded the current stop deadline".into())),
                            None => Err("control companion unavailable".into()),
                        }
                    } else {
                        Err(if !cooperative { "unsupported or unavailable Streamlink control profile" }
                            else { "producer, stdout forwarding, or remux finalization did not finish naturally" }.into())
                    };
                    if let Err(reason) = drain {
                        warn!(download_id = %budget.handle.id, %reason, "Streamlink cooperative drain incomplete");
                        outcome = outcome.with_secondary_error(format!("Streamlink cooperative drain incomplete: {reason}"));
                    }
                    (apply_pipe_failure(outcome, process_pipe_failure.lock().clone()), streamlink_confirmed && ffmpeg_confirmed)
                }
                streamlink_result = async {
                    #[cfg(test)]
                    if let Some(fixture) = &wait_fixture {
                        fixture.inject_wait_failure().await.map_err(|source| process_utils::ContainmentError::Io {
                            operation: "wait for streamlink fixture",
                            source,
                        })?;
                    }
                    streamlink.wait().await
                } => {
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
                                PROCESS_CLEANUP_TIMEOUT, &budget).await {
                                Ok(_) => (Some(failure), true),
                                Err(cleanup_error) => (
                                    Some(failure.with_cleanup_error(cleanup_error)),
                                    false,
                                ),
                            }
                        }
                    };
                    #[cfg(test)]
                    if let Some(fixture) = &shutdown_fixture { fixture.settling_after_source_exit(); }
                    let (ffmpeg_result, ffmpeg_confirmed) = wait_then_terminate(
                        &mut ffmpeg,
                        "ffmpeg",
                        ffmpeg_stop_timeout(),
                        &budget,
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
                    } else {
                        apply_pipe_failure(ffmpeg_outcome, process_pipe_failure.lock().clone())
                    };
                    (outcome, streamlink_confirmed && ffmpeg_confirmed)
                }
                ffmpeg_result = ffmpeg.wait() => match ffmpeg_result {
                    // FFmpeg finalized the output on its own. Reaching this branch before
                    // `streamlink.wait()` is the normal ordering at a clean stream end:
                    // streamlink closes stdout first, the pipe task drops FFmpeg's stdin, and
                    // FFmpeg's stream-copy finalization outruns streamlink's own shutdown. Let
                    // streamlink finish exiting before deciding the outcome instead of reaping
                    // it here.
                    Ok(status) if status.success() => {
                        #[cfg(test)]
                        if let Some(fixture) = &shutdown_fixture { fixture.settling_after_ffmpeg_exit(); }
                        let (settlement, streamlink_cleanup) = settle_streamlink_after_ffmpeg_exit(
                            &mut streamlink,
                            STREAMLINK_SETTLE_TIMEOUT, &budget).await;
                        let mut outcome = clean_ffmpeg_exit_outcome(
                            &settlement,
                            process_pipe_failure.lock().clone(),
                        );
                        let streamlink_confirmed = match streamlink_cleanup {
                            Ok(()) => true,
                            Err(cleanup_error) => {
                                warn!(%cleanup_error, "Failed to stop Streamlink after FFmpeg exited");
                                outcome = outcome.with_cleanup_error(cleanup_error);
                                false
                            }
                        };
                        (outcome, streamlink_confirmed)
                    }
                    ffmpeg_result => {
                        // FFmpeg exited non-zero or could not be waited on, so the pipeline is
                        // already broken and streamlink has nothing left to write into.
                        let (mut ffmpeg_outcome, ffmpeg_confirmed) = match ffmpeg_result {
                            Ok(status) => (StreamlinkPipelineExit::Ffmpeg(status.code()), true),
                            Err(error) => {
                                let failure = StreamlinkPipelineExit::Failed {
                                    kind: DownloadFailureKind::ProcessExit { code: None },
                                    message: format!("Failed to wait for FFmpeg: {error}"),
                                };
                                match terminate_and_reap(
                                    &mut ffmpeg,
                                    "ffmpeg",
                                    PROCESS_CLEANUP_TIMEOUT, &budget).await {
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
                            PROCESS_CLEANUP_TIMEOUT, &budget).await {
                            Ok(_) => true,
                            Err(cleanup_error) => {
                                warn!(%cleanup_error, "Failed to stop Streamlink after FFmpeg exited");
                                ffmpeg_outcome = ffmpeg_outcome.with_cleanup_error(cleanup_error);
                                false
                            }
                        };

                        (
                            apply_pipe_failure(ffmpeg_outcome, process_pipe_failure.lock().clone()),
                            streamlink_confirmed && ffmpeg_confirmed,
                        )
                    }
                }
            };

            #[cfg(test)]
            let (pipeline_exit, cleanup_confirmed) = if inject_unconfirmed_cleanup {
                (
                    StreamlinkPipelineExit::Failed {
                        kind: DownloadFailureKind::ProcessExit { code: None },
                        message: "injected cleanup failure".to_owned(),
                    },
                    false,
                )
            } else {
                (pipeline_exit, cleanup_confirmed)
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

        let streamer_id = config.streamer_id.clone();

        // Spawn task to pipe streamlink stdout to ffmpeg stdin
        let pipe_forced_settlement = forced_settlement.clone();
        let writer_pipe_failure = pipe_failure;
        let pipe_task = AbortOnDropHandle::new(tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = [0u8; 8192];

            loop {
                tokio::select! {
                    _ = pipe_forced_settlement.cancelled() => {
                        warn!("Stopping Streamlink stdout pipe after unconfirmed process cleanup");
                        break;
                    }
                    result = streamlink_stdout.read(&mut buffer) => {
                        match result {
                            Ok(0) => {
                                pipe_eof.store(true, Ordering::Release);
                                break;
                            }
                            Ok(n) => {
                                let written = tokio::select! {
                                    _ = pipe_forced_settlement.cancelled() => break,
                                    written = ffmpeg_stdin.write_all(&buffer[..n]) => written,
                                };
                                if let Err(error) = written {
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
        let stderr_forced_settlement = forced_settlement.clone();
        let streamlink_stderr_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let reader = BufReader::new(streamlink_stderr);
            let mut lines = reader.lines();

            loop {
                tokio::select! {
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
        let events = FfmpegEvents {
            source: FfmpegSource::Streamlink,
            segment_mode,
            single_output_path,
            started_instant,
            streamer_id,
            output_dir: config.output_dir.clone(),
            event_tx: handle.event_tx.clone(),
            forced_settlement: forced_settlement.clone(),
        };
        let event_task = AbortOnDropHandle::new(tokio::spawn(events.run(ffmpeg_stderr, exit_rx)));
        settle_engine_tasks(
            process_task,
            forced_settlement,
            FfmpegSource::Streamlink,
            vec![
                ("stdout pipe", pipe_task),
                ("stderr monitor", streamlink_stderr_task),
                ("event reader", event_task),
            ],
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

    #[tokio::test(start_paused = true)]
    async fn tightened_budget_bounds_negotiation_and_proof_without_restarting_the_future() {
        for source in [true, false] {
            let (events, _receiver) = tokio::sync::mpsc::channel(1);
            let handle = Arc::new(DownloadHandle::new(
                "budget",
                EngineType::Streamlink,
                DownloadConfig::new(
                    "https://invalid.test/",
                    ".",
                    "streamer",
                    "Streamer",
                    "session",
                ),
                events,
            ));
            handle.cancellation_token.cancel();
            let budget = StopBudget {
                handle: handle.clone(),
                configured: Duration::from_secs(60),
                observed: Mutex::new(None),
            };
            let entered = std::sync::atomic::AtomicUsize::new(0);
            let start = Instant::now();
            let mut waiting = Box::pin(budget.within(
                async {
                    entered.fetch_add(1, Ordering::Relaxed);
                    std::future::pending::<()>().await;
                },
                start + Duration::from_secs(60),
                source,
            ));
            assert!(futures::poll!(&mut waiting).is_pending());
            handle.set_stop_deadline(start + Duration::from_millis(100));
            tokio::time::advance(Duration::from_millis(101)).await;
            assert!(waiting.await.is_none());
            assert_eq!(Instant::now(), start + Duration::from_millis(101));
            assert_eq!(entered.load(Ordering::Relaxed), 1);
        }
    }
    use crate::downloader::engine::utils::parse_time;

    #[test]
    fn ffmpeg_path_json_is_optional_and_round_trips() {
        for json in [
            r#"{}"#,
            r#"{"binary_path":"streamlink","ffmpeg_path":null}"#,
        ] {
            let config: StreamlinkEngineConfig = serde_json::from_str(json).unwrap();
            assert!(config.ffmpeg_path.is_none());
            assert!(
                serde_json::to_value(config)
                    .unwrap()
                    .get("ffmpeg_path")
                    .is_none()
            );
        }
        let json = serde_json::json!({"ffmpeg_path": "custom tools/ffmpeg"});
        let config: StreamlinkEngineConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.ffmpeg_path.as_deref(), Some("custom tools/ffmpeg"));
        assert_eq!(
            serde_json::to_value(config).unwrap()["ffmpeg_path"],
            "custom tools/ffmpeg"
        );
        assert!(serde_json::from_str::<StreamlinkEngineConfig>(r#"{"ffmpeg_path":7}"#).is_err());
    }

    #[test]
    fn ffmpeg_command_selection_child() {
        let Ok(json) = std::env::var("SREC_STREAMLINK_CONFIG_FIXTURE") else {
            return;
        };
        let config = serde_json::from_str(&json).unwrap();
        let engine = StreamlinkEngine::with_version(config, None);
        let args = vec!["-i".to_string(), "pipe:0".to_string()];
        let command = engine.ffmpeg_command(&args);
        let expected = std::env::var("SREC_STREAMLINK_EXPECTED_FFMPEG").unwrap();
        assert_eq!(
            command.as_std().get_program(),
            std::ffi::OsStr::new(&expected)
        );
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["-i", "pipe:0"]
        );
    }

    #[tokio::test]
    async fn ffmpeg_command_uses_configured_environment_then_default_executable() {
        for (json, environment, expected) in [
            (
                r#"{"ffmpeg_path":"configured tools/ffmpeg"}"#,
                Some("environment tools/ffmpeg"),
                "configured tools/ffmpeg",
            ),
            (
                r#"{"ffmpeg_path":"configured tools/ffmpeg"}"#,
                None,
                "configured tools/ffmpeg",
            ),
            (
                r#"{"binary_path":"custom-streamlink"}"#,
                Some("environment tools/ffmpeg"),
                "environment tools/ffmpeg",
            ),
            (
                r#"{"ffmpeg_path":null}"#,
                Some("environment tools/ffmpeg"),
                "environment tools/ffmpeg",
            ),
            (r#"{}"#, None, "ffmpeg"),
            (r#"{"ffmpeg_path":null}"#, None, "ffmpeg"),
            (r#"{"ffmpeg_path":""}"#, Some("environment-ffmpeg"), ""),
            (
                r#"{"ffmpeg_path":"   "}"#,
                Some("environment-ffmpeg"),
                "   ",
            ),
            (r#"{}"#, Some(""), ""),
        ] {
            let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "downloader::engine::streamlink::tests::ffmpeg_command_selection_child",
                    "--nocapture",
                ])
                .env("SREC_STREAMLINK_CONFIG_FIXTURE", json)
                .env("SREC_STREAMLINK_EXPECTED_FFMPEG", expected)
                .env_remove("FFMPEG_PATH")
                .kill_on_drop(true);
            if let Some(value) = environment {
                command.env("FFMPEG_PATH", value);
            }
            let output = tokio::time::timeout(Duration::from_secs(10), command.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "selection case {json}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "selection fixture must execute exactly one test"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_error_precedes_terminal_in_streamlink_pipeline() {
        use super::super::utils::test_support::{assert_output_failure, script};
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg_path = script(
            dir.path(),
            "ffmpeg",
            "printf '%s\\n' 'Error opening output file: Permission denied' >&2\nexit 0\n",
        );
        let binary_path = script(dir.path(), "streamlink", "sleep 1\nexit 0\n");
        let engine = StreamlinkEngine {
            config: StreamlinkEngineConfig {
                binary_path,
                ..Default::default()
            },
            ffmpeg_path,
            version: None,
            fixture: None,
            shutdown_fixture: None,
        };
        assert_output_failure(&engine, dir.path()).await;
    }
    #[test]
    fn test_engine_type() {
        let engine = StreamlinkEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Streamlink);
    }

    #[test]
    fn remux_logging_and_literal_percent_paths_preserve_recording_contract() {
        let engine = StreamlinkEngine {
            config: StreamlinkEngineConfig::default(),
            ffmpeg_path: String::new(),
            version: None,
            fixture: None,
            shutdown_fixture: None,
        };
        for segment_duration in [0, 10] {
            let config = DownloadConfig::new(
                "https://invalid.test/live",
                "root/50%d",
                "id",
                "Name",
                "session",
            )
            .with_filename_template(crate::utils::filename::sanitize_filename_for_template(
                "name%Y%n",
            ))
            .with_output_format("ts")
            .with_max_segment_duration(segment_duration);
            let args = engine.build_ffmpeg_args(&config);
            assert_eq!(
                &args[args.len() - 4..args.len() - 1],
                ["-loglevel", "info", "-stats"]
            );
            let path = args.last().unwrap();
            if segment_duration == 0 {
                assert_eq!(path, "root/50%d/name%Y%n.ts");
            } else {
                assert_eq!(
                    pipeline_common::expand_path_template(path),
                    "root/50%d/name%Y%n.ts"
                );
            }
        }
    }

    #[tokio::test]
    async fn recording_stop_settlement() {
        use super::super::utils::test_support::{
            RecordingFixture, StopCase, assert_recording_stop,
        };
        for case in StopCase::ALL {
            let dir = tempfile::tempdir().unwrap();
            let fixture = RecordingFixture::new(dir.path(), case);
            let engine = StreamlinkEngine {
                config: StreamlinkEngineConfig::default(),
                ffmpeg_path: String::new(),
                version: None,
                fixture: Some(fixture.clone()),
                shutdown_fixture: None,
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
            let engine = StreamlinkEngine {
                config: StreamlinkEngineConfig::default(),
                ffmpeg_path: "unused-fixture".into(),
                version: None,
                fixture: Some(RecordingFixture::contract(directory.path(), case, true)),
                shutdown_fixture: None,
            };
            assert_recording_contract(engine, directory.path(), case).await;
        }
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

    #[test]
    fn clean_ffmpeg_exit_completes_when_streamlink_also_exits_cleanly() {
        let outcome = clean_ffmpeg_exit_outcome(&StreamlinkSettlement::ExitedCleanly, None);
        assert!(matches!(outcome, StreamlinkPipelineExit::Ffmpeg(Some(0))));
    }

    #[test]
    fn clean_ffmpeg_exit_reports_a_non_zero_streamlink_status_as_failure() {
        let outcome = clean_ffmpeg_exit_outcome(
            &StreamlinkSettlement::ExitedWithFailure {
                code: Some(130),
                status: "exit code: 130".to_string(),
            },
            None,
        );

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::ProcessExit { code: Some(130) });
        assert!(message.contains("exit code: 130"));
    }

    #[test]
    fn clean_ffmpeg_exit_reports_a_streamlink_wait_error_as_failure() {
        let outcome = clean_ffmpeg_exit_outcome(
            &StreamlinkSettlement::WaitFailed("no child processes".to_string()),
            None,
        );

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::ProcessExit { code: None });
        assert!(message.contains("no child processes"));
    }

    #[test]
    fn clean_ffmpeg_exit_fails_when_streamlink_outlives_the_grace_window() {
        let outcome = clean_ffmpeg_exit_outcome(&StreamlinkSettlement::StillRunning, None);

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::Other);
        assert_eq!(message, "FFmpeg exited before Streamlink completed");
    }

    #[test]
    fn pipe_failure_outranks_a_clean_ffmpeg_exit() {
        let outcome = clean_ffmpeg_exit_outcome(
            &StreamlinkSettlement::ExitedCleanly,
            Some("Failed to read Streamlink output: broken pipe".to_string()),
        );

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::Network);
        assert!(message.contains("broken pipe"));
        // A zero FFmpeg status has no `failure_summary`, so nothing is attached.
        assert!(!message.contains("secondary process error"));
    }

    #[test]
    fn streamlink_failure_outranks_a_recorded_pipe_failure() {
        let outcome = clean_ffmpeg_exit_outcome(
            &StreamlinkSettlement::ExitedWithFailure {
                code: Some(1),
                status: "exit code: 1".to_string(),
            },
            Some("Failed to read Streamlink output: broken pipe".to_string()),
        );

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::ProcessExit { code: Some(1) });
        assert!(message.contains("exit code: 1"));
    }

    #[test]
    fn pipe_failure_keeps_a_non_zero_ffmpeg_exit_as_secondary_error() {
        let outcome = apply_pipe_failure(
            StreamlinkPipelineExit::Ffmpeg(Some(1)),
            Some("Failed to pipe Streamlink output into FFmpeg: broken pipe".to_string()),
        );

        let StreamlinkPipelineExit::Failed { kind, message } = outcome else {
            panic!("expected failure outcome");
        };
        assert_eq!(kind, DownloadFailureKind::Network);
        assert!(message.contains("broken pipe"));
        assert!(message.contains("FFmpeg exited with code 1"));
    }

    #[test]
    fn absent_pipe_failure_leaves_the_outcome_untouched() {
        let outcome = apply_pipe_failure(StreamlinkPipelineExit::Ffmpeg(Some(0)), None);
        assert!(matches!(outcome, StreamlinkPipelineExit::Ffmpeg(Some(0))));
    }
}
