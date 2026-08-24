use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use process_utils::{ContainedChild, TreeTerminator};
use sqlx::sqlite::SqliteConnectOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;
use tokio::time::timeout_at;
use tracing::{error, warn};

use super::deadline::{HardDeadlineWatchdog, ShutdownPolicy, ShutdownSchedule};
use super::generation::{DirtyGenerationMarker, RuntimeGeneration, RuntimeLease};
use crate::{Error, Result};

const RUNTIME_ROLE_ENV: &str = "RUST_SREC_RUNTIME_ROLE";
const RUNTIME_WORKER_ROLE: &str = "worker";
const RUNTIME_GENERATION_ENV: &str = "RUST_SREC_RUNTIME_GENERATION";
const PREVIOUS_DIRTY_GENERATION_ENV: &str = "RUST_SREC_PREVIOUS_DIRTY_GENERATION";
const RUNTIME_MARKER_PATH_ENV: &str = "RUST_SREC_RUNTIME_MARKER_PATH";
const SHUTDOWN_TIMEOUT_ENV: &str = "RUST_SREC_SHUTDOWN_TIMEOUT_SECS";
const SHUTDOWN_FORCE_RESERVE_ENV: &str = "RUST_SREC_SHUTDOWN_FORCE_RESERVE_SECS";
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_FORCE_RESERVE: Duration = Duration::from_secs(2);
const HARD_DEADLINE_EXIT_CODE: i32 = 124;
const HARD_DEADLINE_CONTAINMENT_FAILURE_EXIT_CODE: i32 = 125;

struct ProcessExitGuard {
    exit_code: i32,
}

impl Drop for ProcessExitGuard {
    fn drop(&mut self) {
        std::process::exit(self.exit_code);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerShutdownReason {
    Signal,
    SupervisorDisconnected,
}

impl WorkerShutdownReason {
    fn protocol_token(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::SupervisorDisconnected => "supervisor_disconnected",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Signal => "Signal received",
            Self::SupervisorDisconnected => "Runtime supervisor disconnected",
        }
    }
}

impl FromStr for WorkerShutdownReason {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "signal" => Ok(Self::Signal),
            "supervisor_disconnected" => Ok(Self::SupervisorDisconnected),
            _ => Err(Error::validation(format!(
                "unknown runtime shutdown reason '{value}'"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeTermination {
    Clean,
    CleanRecoveryPending,
    ForcedRecoveryPending,
    CrashedRecoveryPending,
}

#[derive(Debug)]
pub(crate) struct RuntimeExitReport {
    pub(crate) termination: RuntimeTermination,
    pub(crate) generation: RuntimeGeneration,
    pub(crate) exit_code: Option<i32>,
    pub(crate) elapsed: Duration,
    pub(crate) marker_error: Option<String>,
}

pub(crate) struct SupervisedRun {
    outcome: Result<RuntimeExitReport>,
    _shutdown_monitor: ShutdownMonitor,
}

impl SupervisedRun {
    /// Report the terminal outcome and exit without waiting for Tokio runtime
    /// teardown. The monitor remains armed while `report` performs diagnostics.
    pub(crate) fn finish<F>(self, report: F) -> !
    where
        F: FnOnce(&Result<RuntimeExitReport>),
    {
        let exit_code = match &self.outcome {
            Ok(report)
                if matches!(
                    report.termination,
                    RuntimeTermination::Clean | RuntimeTermination::CleanRecoveryPending
                ) =>
            {
                0
            }
            Ok(_) | Err(_) => 1,
        };
        let _exit_guard = ProcessExitGuard { exit_code };
        report(&self.outcome);
        std::process::exit(exit_code);
    }

    #[cfg(test)]
    fn outcome(&self) -> &Result<RuntimeExitReport> {
        &self.outcome
    }
}

#[derive(Clone, Copy, Debug)]
struct ShutdownRequest {
    reason: WorkerShutdownReason,
    started_at: Instant,
    schedule: ShutdownSchedule,
}

/// Observes process shutdown independently from async supervisor progress.
///
/// This thread starts before filesystem admission work. Once a signal is
/// observed it arms the absolute watchdog immediately and retains that
/// watchdog through result reporting and the terminal process exit.
struct ShutdownMonitor {
    request_rx: oneshot::Receiver<ShutdownRequest>,
    cancel_tx: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
    terminator: Arc<OnceLock<TreeTerminator>>,
    watchdog: Arc<Mutex<Option<HardDeadlineWatchdog>>>,
    disarm_watchdog_on_drop: bool,
}

impl ShutdownMonitor {
    fn start<F>(shutdown: F, policy: ShutdownPolicy) -> Result<Self>
    where
        F: Future<Output = WorkerShutdownReason> + Send + 'static,
    {
        Self::start_with_drop_policy(shutdown, policy, false)
    }

    #[cfg(test)]
    fn start_for_test<F>(shutdown: F, policy: ShutdownPolicy) -> Result<Self>
    where
        F: Future<Output = WorkerShutdownReason> + Send + 'static,
    {
        Self::start_with_drop_policy(shutdown, policy, true)
    }

    fn start_with_drop_policy<F>(
        shutdown: F,
        policy: ShutdownPolicy,
        disarm_watchdog_on_drop: bool,
    ) -> Result<Self>
    where
        F: Future<Output = WorkerShutdownReason> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            Error::Other(format!(
                "shutdown monitor requires an active Tokio runtime: {error}"
            ))
        })?;
        let (request_tx, request_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let terminator = Arc::new(OnceLock::<TreeTerminator>::new());
        let watchdog = Arc::new(Mutex::new(None::<HardDeadlineWatchdog>));
        let deadline_terminator = Arc::clone(&terminator);
        let monitor_watchdog = Arc::clone(&watchdog);

        let worker = thread::Builder::new()
            .name("shutdown-signal-monitor".to_string())
            .spawn(move || {
                let reason = runtime.block_on(async move {
                    tokio::pin!(shutdown);
                    tokio::select! {
                        biased;
                        reason = &mut shutdown => Some(reason),
                        _ = cancel_rx => None,
                    }
                });
                let Some(reason) = reason else {
                    return;
                };

                let started_at = Instant::now();
                let schedule = match ShutdownSchedule::from_start(started_at, policy) {
                    Ok(schedule) => schedule,
                    Err(_) => {
                        std::process::exit(HARD_DEADLINE_EXIT_CODE);
                    }
                };
                let watchdog =
                    match HardDeadlineWatchdog::arm(schedule.deadline(), move || {
                        let _exit_guard = ProcessExitGuard {
                            exit_code: HARD_DEADLINE_EXIT_CODE,
                        };
                        let exit_code = deadline_terminator.get().map_or(
                            HARD_DEADLINE_EXIT_CODE,
                            |terminator| match terminator.terminate_tree() {
                                Ok(()) => HARD_DEADLINE_EXIT_CODE,
                                // Blocking diagnostics are unsafe at the terminal
                                // deadline, so the process status exposes failure.
                                Err(_) => HARD_DEADLINE_CONTAINMENT_FAILURE_EXIT_CODE,
                            },
                        );
                        std::process::exit(exit_code);
                    }) {
                        Ok(watchdog) => watchdog,
                        Err(_) => {
                            std::process::exit(HARD_DEADLINE_EXIT_CODE);
                        }
                    };
                *lock_unpoisoned(&monitor_watchdog) = Some(watchdog);

                // A closed receiver means the supervisor is already exiting.
                // The watchdog remains owned by the monitor until then.
                if request_tx
                    .send(ShutdownRequest {
                        reason,
                        started_at,
                        schedule,
                    })
                    .is_err()
                {
                    tracing::debug!("Supervisor exited before receiving its shutdown request");
                }
            })
            .map_err(|error| {
                Error::io_path(
                    "starting shutdown signal monitor",
                    Path::new("<thread>"),
                    error,
                )
            })?;

        Ok(Self {
            request_rx,
            cancel_tx: Some(cancel_tx),
            worker: Some(worker),
            terminator,
            watchdog,
            disarm_watchdog_on_drop,
        })
    }

    fn install_terminator(&self, terminator: TreeTerminator) -> Result<()> {
        self.terminator.set(terminator).map_err(|_| {
            Error::Other("runtime terminator was installed more than once".to_string())
        })
    }

    async fn wait_for_shutdown(&mut self) -> Result<ShutdownRequest> {
        (&mut self.request_rx)
            .await
            .map_err(|_| Error::Other("shutdown signal monitor stopped unexpectedly".to_string()))
    }
}

impl Drop for ShutdownMonitor {
    fn drop(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take()
            && cancel_tx.send(()).is_err()
        {
            tracing::debug!("Shutdown signal monitor was already settled during cancellation");
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            error!("Shutdown signal monitor thread panicked while settling");
        }
        if let Some(watchdog) = lock_unpoisoned(&self.watchdog).take() {
            if self.disarm_watchdog_on_drop {
                settle_watchdog(watchdog);
            } else {
                // Production unwinding after signal observation must stay
                // fail-closed. Forgetting detaches the watchdog JoinHandle and
                // retains its disarm sender until the deadline callback exits.
                std::mem::forget(watchdog);
            }
        }
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn is_worker_process() -> bool {
    std::env::var_os(RUNTIME_ROLE_ENV).is_some_and(|role| role == RUNTIME_WORKER_ROLE)
}

pub(crate) async fn supervise_current_executable<F>(shutdown: F) -> Result<SupervisedRun>
where
    F: Future<Output = WorkerShutdownReason> + Send + 'static,
{
    let policy = shutdown_policy_from_environment()?;
    let mut shutdown_monitor = ShutdownMonitor::start(shutdown, policy)?;
    let outcome = async {
        let marker_path = runtime_marker_path()?;
        let command = current_worker_command()?;
        supervise_command_with_monitor(command, marker_path, &mut shutdown_monitor).await
    }
    .await;
    Ok(SupervisedRun {
        outcome,
        _shutdown_monitor: shutdown_monitor,
    })
}

async fn supervise_command_with_monitor(
    mut command: Command,
    marker_path: PathBuf,
    shutdown_monitor: &mut ShutdownMonitor,
) -> Result<RuntimeExitReport> {
    if let Some(parent) = marker_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::io_path("creating runtime state directory", parent, error))?;
    }
    let _runtime_lease = RuntimeLease::acquire(&marker_path)?;

    let previous_state = DirtyGenerationMarker::load(&marker_path)?;
    let previous_generation = previous_state
        .as_ref()
        .and_then(|state| state.latest_dirty_generation());
    let previous_dirty_count = previous_state
        .as_ref()
        .map_or(0, |state| state.dirty_generation_count());
    if let Some(previous_generation) = previous_generation {
        eprintln!(
            "Runtime recovery ledger contains {previous_dirty_count} unresolved generation(s), most recently {previous_generation}"
        );
    }

    let generation = RuntimeGeneration::generate();
    command
        .env(RUNTIME_ROLE_ENV, RUNTIME_WORKER_ROLE)
        .env(RUNTIME_GENERATION_ENV, generation.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(previous_generation) = previous_generation {
        command.env(
            PREVIOUS_DIRTY_GENERATION_ENV,
            previous_generation.to_string(),
        );
    }
    let mut child = ContainedChild::spawn(&mut command)
        .map_err(|error| Error::Other(format!("failed to launch contained runtime: {error}")))?;
    shutdown_monitor.install_terminator(child.tree_terminator())?;
    let mut control = child.take_stdin().ok_or_else(|| {
        Error::Other("contained runtime did not expose its control pipe".to_string())
    })?;

    // The worker waits for START before opening SQLite or output files. Install
    // the durable dirty marker only after OS containment is confirmed, then
    // release worker admission.
    let marker = match DirtyGenerationMarker::begin(&marker_path, generation) {
        Ok(marker) => marker,
        Err(error) => {
            terminate_failed_launch(&mut child).await;
            return Err(error);
        }
    };
    if let Err(error) = write_control_line(&mut control, &format!("START {generation}")).await {
        terminate_failed_launch(&mut child).await;
        return Err(Error::io_path(
            "starting isolated runtime worker",
            &marker_path,
            error,
        ));
    }

    tokio::select! {
        status = child.wait() => {
            let status = status.map_err(|error| {
                Error::Other(format!("failed waiting for contained runtime: {error}"))
            })?;
            classify_settled_exit(marker, status, Duration::ZERO, false)
        }
        request = shutdown_monitor.wait_for_shutdown() => {
            shutdown_contained_runtime(child, control, marker, request?).await
        }
    }
}

#[cfg(test)]
async fn supervise_command<F>(
    command: Command,
    policy: ShutdownPolicy,
    marker_path: PathBuf,
    shutdown: F,
) -> Result<SupervisedRun>
where
    F: Future<Output = WorkerShutdownReason> + Send + 'static,
{
    let mut shutdown_monitor = ShutdownMonitor::start_for_test(shutdown, policy)?;
    let outcome = supervise_command_with_monitor(command, marker_path, &mut shutdown_monitor).await;
    Ok(SupervisedRun {
        outcome,
        _shutdown_monitor: shutdown_monitor,
    })
}

fn current_worker_command() -> Result<Command> {
    let executable = std::env::current_exe().map_err(|error| {
        Error::io_path(
            "resolving the runtime supervisor executable",
            Path::new("<current executable>"),
            error,
        )
    })?;
    let mut command = Command::new(executable);
    command.args(std::env::args_os().skip(1));
    Ok(command)
}

async fn terminate_failed_launch(child: &mut ContainedChild) {
    let deadline = tokio::time::Instant::now() + DEFAULT_FORCE_RESERVE;
    if let Err(error) = child.terminate_tree_until(deadline).await {
        error!(%error, "Failed to contain runtime after launch error");
    }
}

async fn shutdown_contained_runtime(
    mut child: ContainedChild,
    mut control: ChildStdin,
    marker: DirtyGenerationMarker,
    request: ShutdownRequest,
) -> Result<RuntimeExitReport> {
    if let Err(error) = write_control_line(
        &mut control,
        &format!("SHUTDOWN {}", request.reason.protocol_token()),
    )
    .await
    {
        warn!(%error, "Runtime control pipe closed before shutdown request was delivered");
    }
    drop(control);

    let force_at = tokio::time::Instant::from_std(request.schedule.force_at());
    match timeout_at(force_at, child.wait()).await {
        Ok(Ok(status)) => {
            classify_settled_exit(marker, status, request.started_at.elapsed(), false)
        }
        Ok(Err(error)) => {
            warn!(%error, "Contained runtime wait failed; forcing process tree");
            force_contained_runtime(child, marker, request.schedule, request.started_at).await
        }
        Err(_) => {
            force_contained_runtime(child, marker, request.schedule, request.started_at).await
        }
    }
}

async fn force_contained_runtime(
    mut child: ContainedChild,
    marker: DirtyGenerationMarker,
    schedule: ShutdownSchedule,
    started_at: Instant,
) -> Result<RuntimeExitReport> {
    let hard_deadline = tokio::time::Instant::from_std(schedule.deadline());
    match child.terminate_tree_until(hard_deadline).await {
        Ok(status) => classify_settled_exit(marker, status, started_at.elapsed(), true),
        Err(error) => {
            error!(%error, "Whole runtime process tree did not settle before its hard deadline");
            // Returning would violate the containment interface. Keep the
            // child guard alive; the supervisor-owned watchdog fail-stops this
            // process at the already established absolute deadline.
            let _child = child;
            let _marker = marker;
            std::future::pending::<Result<RuntimeExitReport>>().await
        }
    }
}

fn classify_settled_exit(
    marker: DirtyGenerationMarker,
    status: ExitStatus,
    elapsed: Duration,
    forced: bool,
) -> Result<RuntimeExitReport> {
    let generation = marker.generation();
    if forced {
        drop(marker);
        return Ok(RuntimeExitReport {
            termination: RuntimeTermination::ForcedRecoveryPending,
            generation,
            exit_code: status.code(),
            elapsed,
            marker_error: None,
        });
    }

    if status.success() {
        let (termination, marker_error) = match marker.clear() {
            Ok(false) => (RuntimeTermination::Clean, None),
            Ok(true) => (RuntimeTermination::CleanRecoveryPending, None),
            Err(error) => (
                RuntimeTermination::CrashedRecoveryPending,
                Some(error.to_string()),
            ),
        };
        Ok(RuntimeExitReport {
            termination,
            generation,
            exit_code: status.code(),
            elapsed,
            marker_error,
        })
    } else {
        drop(marker);
        Ok(RuntimeExitReport {
            termination: RuntimeTermination::CrashedRecoveryPending,
            generation,
            exit_code: status.code(),
            elapsed,
            marker_error: None,
        })
    }
}

fn settle_watchdog(watchdog: HardDeadlineWatchdog) {
    if watchdog.disarm().is_err() {
        error!("Hard-deadline watchdog thread panicked while settling");
    }
}

async fn write_control_line(control: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    control.write_all(line.as_bytes()).await?;
    control.write_all(b"\n").await?;
    control.flush().await
}

fn shutdown_policy_from_environment() -> Result<ShutdownPolicy> {
    let total = duration_from_environment(SHUTDOWN_TIMEOUT_ENV, DEFAULT_SHUTDOWN_TIMEOUT)?;
    let force_reserve =
        duration_from_environment(SHUTDOWN_FORCE_RESERVE_ENV, DEFAULT_FORCE_RESERVE)?;
    ShutdownPolicy::new(total, force_reserve).map_err(|error| Error::config(error.to_string()))
}

fn duration_from_environment(name: &str, default: Duration) -> Result<Duration> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(default);
    };
    let raw = raw
        .into_string()
        .map_err(|_| Error::config(format!("environment variable {name} is not valid Unicode")))?;
    let seconds = raw.parse::<u64>().map_err(|error| {
        Error::config(format!(
            "environment variable {name} must be an integer number of seconds: {error}"
        ))
    })?;
    Ok(Duration::from_secs(seconds))
}

fn runtime_marker_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(RUNTIME_MARKER_PATH_ENV)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:srec.db?mode=rwc".to_string());
    marker_path_from_database_url(&database_url)
}

fn marker_path_from_database_url(database_url: &str) -> Result<PathBuf> {
    let options = SqliteConnectOptions::from_str(database_url).map_err(|error| {
        Error::config(format!(
            "cannot derive runtime marker path from DATABASE_URL: {error}"
        ))
    })?;
    let database_path = options.get_filename();
    let parent = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut marker_name = database_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("rust-srec"))
        .to_os_string();
    marker_name.push(".runtime-generation.dirty");
    Ok(parent.join(marker_name))
}

pub(crate) struct WorkerControl {
    lines: Lines<BufReader<tokio::io::Stdin>>,
    pub(crate) generation: RuntimeGeneration,
    pub(crate) previous_dirty_generation: Option<RuntimeGeneration>,
}

impl WorkerControl {
    pub(crate) async fn wait_for_shutdown(&mut self) -> Result<WorkerShutdownReason> {
        let Some(line) = self.lines.next_line().await? else {
            return Ok(WorkerShutdownReason::SupervisorDisconnected);
        };
        parse_shutdown_command(&line)
    }
}

pub(crate) async fn wait_for_worker_start() -> Result<WorkerControl> {
    let generation = required_generation_environment(RUNTIME_GENERATION_ENV)?;
    let previous_dirty_generation = optional_generation_environment(PREVIOUS_DIRTY_GENERATION_ENV)?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let line = lines.next_line().await?.ok_or_else(|| {
        Error::Other("runtime supervisor disconnected before worker admission".to_string())
    })?;
    let admitted_generation = parse_start_command(&line)?;
    if admitted_generation != generation {
        return Err(Error::validation(format!(
            "runtime START generation {admitted_generation} does not match worker generation {generation}"
        )));
    }
    Ok(WorkerControl {
        lines,
        generation,
        previous_dirty_generation,
    })
}

fn required_generation_environment(name: &str) -> Result<RuntimeGeneration> {
    let raw = std::env::var(name)
        .map_err(|error| Error::config(format!("missing or invalid {name}: {error}")))?;
    raw.parse()
        .map_err(|error| Error::config(format!("environment variable {name} is invalid: {error}")))
}

fn optional_generation_environment(name: &str) -> Result<Option<RuntimeGeneration>> {
    match std::env::var(name) {
        Ok(raw) => raw.parse().map(Some).map_err(|error| {
            Error::config(format!("environment variable {name} is invalid: {error}"))
        }),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(Error::config(format!(
            "environment variable {name} is invalid: {error}"
        ))),
    }
}

fn parse_start_command(line: &str) -> Result<RuntimeGeneration> {
    let mut fields = line.split_whitespace();
    let command = fields.next();
    let generation = fields.next();
    if command != Some("START") || generation.is_none() || fields.next().is_some() {
        return Err(Error::validation("invalid runtime START command"));
    }
    generation
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::validation("runtime START command has an invalid generation"))
}

fn parse_shutdown_command(line: &str) -> Result<WorkerShutdownReason> {
    let mut fields = line.split_whitespace();
    let command = fields.next();
    let reason = fields.next();
    if command != Some("SHUTDOWN") || reason.is_none() || fields.next().is_some() {
        return Err(Error::validation("invalid runtime SHUTDOWN command"));
    }
    reason
        .ok_or_else(|| Error::validation("runtime SHUTDOWN command is missing a reason"))?
        .parse()
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    const TEST_WORKER_MODE_ENV: &str = "RUST_SREC_TEST_WORKER_MODE";

    #[test]
    fn control_protocol_rejects_malformed_frames() {
        assert!(parse_start_command("START").is_err());
        assert!(parse_start_command("START not-a-uuid").is_err());
        assert!(parse_start_command("START a b").is_err());
        assert!(parse_shutdown_command("SHUTDOWN").is_err());
        assert!(parse_shutdown_command("SHUTDOWN unknown").is_err());
        assert!(parse_shutdown_command("SHUTDOWN signal trailing").is_err());
    }

    #[test]
    fn control_protocol_round_trips_generation_and_reason() {
        let generation = RuntimeGeneration::generate();
        assert_eq!(
            parse_start_command(&format!("START {generation}")).unwrap(),
            generation
        );
        assert_eq!(
            parse_shutdown_command("SHUTDOWN signal").unwrap(),
            WorkerShutdownReason::Signal
        );
    }

    #[test]
    fn default_marker_path_is_adjacent_to_sqlite_database() {
        let marker = marker_path_from_database_url("sqlite:state/srec.db?mode=rwc")
            .expect("SQLite URL should produce a marker path");

        assert_eq!(
            marker,
            PathBuf::from("state/srec.db.runtime-generation.dirty")
        );
    }

    #[tokio::test]
    async fn shutdown_monitor_starts_clock_while_supervisor_progress_is_blocked() {
        let policy = ShutdownPolicy::new(Duration::from_secs(5), Duration::from_secs(1))
            .expect("valid shutdown policy");
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let mut monitor = ShutdownMonitor::start_for_test(
            async move {
                observed_tx
                    .send(())
                    .expect("test should observe shutdown future polling");
                WorkerShutdownReason::Signal
            },
            policy,
        )
        .expect("shutdown monitor should start");

        observed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("monitor should poll independently");
        let armed_by = Instant::now() + Duration::from_secs(1);
        while lock_unpoisoned(&monitor.watchdog).is_none() {
            assert!(
                Instant::now() < armed_by,
                "monitor should arm its watchdog independently"
            );
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(100));
        let request = monitor
            .wait_for_shutdown()
            .await
            .expect("monitor should deliver shutdown request");

        assert!(request.started_at.elapsed() >= Duration::from_millis(100));
        assert_eq!(
            request
                .schedule
                .deadline()
                .duration_since(request.started_at),
            policy.total()
        );
        assert!(lock_unpoisoned(&monitor.watchdog).is_some());
    }

    #[tokio::test]
    #[cfg(any(unix, windows))]
    async fn clean_contained_worker_clears_its_dirty_marker() {
        if run_test_worker_if_requested("clean").await {
            return;
        }

        let directory = tempfile::tempdir().expect("runtime state directory");
        let marker_path = directory.path().join("runtime.dirty");
        let policy = ShutdownPolicy::new(Duration::from_secs(4), Duration::from_secs(2))
            .expect("valid shutdown policy");
        let command = test_worker_command(
            "runtime::supervisor::tests::clean_contained_worker_clears_its_dirty_marker",
            "clean",
        );

        let supervised = supervise_command(command, policy, marker_path.clone(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            WorkerShutdownReason::Signal
        })
        .await
        .expect("shutdown monitor should start");
        let report = supervised
            .outcome()
            .as_ref()
            .expect("clean worker should settle");

        assert_eq!(report.termination, RuntimeTermination::Clean);
        assert!(!marker_path.exists());
    }

    #[tokio::test]
    #[cfg(any(unix, windows))]
    async fn hard_deadline_force_contains_an_unresponsive_worker() {
        if run_test_worker_if_requested("hang").await {
            return;
        }

        let directory = tempfile::tempdir().expect("runtime state directory");
        let marker_path = directory.path().join("runtime.dirty");
        let policy = ShutdownPolicy::new(Duration::from_secs(4), Duration::from_secs(2))
            .expect("valid shutdown policy");
        let command = test_worker_command(
            "runtime::supervisor::tests::hard_deadline_force_contains_an_unresponsive_worker",
            "hang",
        );
        let started_at = Instant::now();

        let supervised = supervise_command(command, policy, marker_path.clone(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            WorkerShutdownReason::Signal
        })
        .await
        .expect("shutdown monitor should start");
        let report = supervised
            .outcome()
            .as_ref()
            .expect("unresponsive worker should be force-contained");

        assert_eq!(
            report.termination,
            RuntimeTermination::ForcedRecoveryPending
        );
        assert!(started_at.elapsed() < Duration::from_secs(4));
        assert!(marker_path.exists());
    }

    #[tokio::test]
    #[cfg(any(unix, windows))]
    async fn fail_stopped_worker_retains_its_dirty_marker() {
        if run_test_worker_if_requested("crash").await {
            return;
        }

        let directory = tempfile::tempdir().expect("runtime state directory");
        let marker_path = directory.path().join("runtime.dirty");
        let policy = ShutdownPolicy::new(Duration::from_secs(4), Duration::from_secs(2))
            .expect("valid shutdown policy");
        let command = test_worker_command(
            "runtime::supervisor::tests::fail_stopped_worker_retains_its_dirty_marker",
            "crash",
        );

        let supervised = supervise_command(
            command,
            policy,
            marker_path.clone(),
            std::future::pending::<WorkerShutdownReason>(),
        )
        .await
        .expect("shutdown monitor should start");
        let report = supervised
            .outcome()
            .as_ref()
            .expect("parent should observe the fail-stopped worker");

        assert_eq!(
            report.termination,
            RuntimeTermination::CrashedRecoveryPending
        );
        assert_eq!(report.exit_code, Some(70));
        assert!(marker_path.exists());
    }

    #[cfg(any(unix, windows))]
    async fn run_test_worker_if_requested(expected_mode: &str) -> bool {
        let Ok(mode) = std::env::var(TEST_WORKER_MODE_ENV) else {
            return false;
        };
        if mode != expected_mode {
            return false;
        }

        let mut control = wait_for_worker_start()
            .await
            .expect("test worker receives START");
        match mode.as_str() {
            "clean" => {
                assert_eq!(
                    control
                        .wait_for_shutdown()
                        .await
                        .expect("test worker receives SHUTDOWN"),
                    WorkerShutdownReason::Signal
                );
            }
            "hang" => std::future::pending::<()>().await,
            "crash" => std::process::exit(70),
            _ => unreachable!("test helper mode was checked by its caller"),
        }
        true
    }

    #[cfg(any(unix, windows))]
    fn test_worker_command(test_name: &str, mode: &str) -> Command {
        let executable = std::env::current_exe().expect("test executable");
        let mut command = Command::new(executable);
        command
            .arg(test_name)
            .arg("--exact")
            .arg("--nocapture")
            .env(TEST_WORKER_MODE_ENV, mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}
