use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rust_srec::backend;
use tauri::Manager;

use crate::boot_error::BootProgressPayload;
use crate::desktop_notifications::DesktopNotificationConfig;

/// Cooperative drain budget handed to `ServiceContainer::shutdown_with_hard_cap`.
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// Wall-clock cap on the whole in-process drain.
///
/// The desktop hosts the backend inside its own process, so nothing outside it
/// can force-kill a wedged engine or pipeline job. Past this cap
/// `shutdown_with_hard_cap` aborts the remaining tasks — which is what kills
/// their ffmpeg/streamlink children — so `app_handle.exit` cannot orphan them.
pub(crate) const SHUTDOWN_HARD_CAP: Duration = Duration::from_secs(60);

/// Wall-clock cap on waiting for the boot task before the exit path contains
/// the backend.
///
/// `ServiceContainer::initialize` is not cancellable and must not overlap
/// `shutdown_with_hard_cap`, or work spawned after the drain would outlive it.
/// An exit requested while services are starting therefore waits for the boot
/// task to return. Past this cap the exit path proceeds anyway: a boot wedged
/// that long is already beyond cooperative cleanup.
pub(crate) const BOOT_SETTLE_CAP: Duration = Duration::from_secs(60);

/// Boot progress as seen by the exit path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootPhase {
    /// Nothing that can own a recording process has been started.
    Preparing,
    /// `ServiceContainer::initialize` has been entered: actors, downloads and
    /// pipeline workers may exist from here on.
    Services,
    /// The boot task returned; the exit path owns the container from here on.
    Settled,
}

/// Outcome of waiting for the boot task before containing the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootSettle {
    /// The boot task never entered `BootPhase::Services`, and once cancelled it
    /// cannot enter it any more.
    NotStarted,
    Settled,
    /// The boot task ended without settling (a panic dropped its sender).
    Abandoned,
    TimedOut,
}

/// Wait until the boot task can no longer be inside
/// `ServiceContainer::initialize`.
///
/// Caller contract: cancel the boot task (`DesktopBackendState::init_cancel`)
/// before calling this. The boot task publishes `BootPhase::Services` before it
/// checks that token, so a `Preparing` reading here proves the boot task will
/// observe the cancellation and return before starting services.
pub(crate) async fn wait_for_boot_settled(
    mut boot_phase: tokio::sync::watch::Receiver<BootPhase>,
    cap: Duration,
) -> BootSettle {
    if *boot_phase.borrow() == BootPhase::Preparing {
        return BootSettle::NotStarted;
    }
    match tokio::time::timeout(
        cap,
        boot_phase.wait_for(|phase| *phase == BootPhase::Settled),
    )
    .await
    {
        Ok(Ok(_)) => BootSettle::Settled,
        Ok(Err(_)) => BootSettle::Abandoned,
        Err(_) => BootSettle::TimedOut,
    }
}

pub struct DesktopBackendState {
    container: std::sync::Mutex<Option<Arc<backend::ServiceContainer>>>,
    log_guard: std::sync::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
    _instance_lock: Option<backend::RuntimeLease>,
    boot_failed: AtomicBool,
    pub(crate) shutdown_started: AtomicBool,
    /// Set once the exit owner has drained the backend (`finish_exit`,
    /// `restart_desktop` just before its restart, or the `Exit` arm after
    /// draining an AppKit termination). While `shutdown_started` is set and
    /// this is not, the `ExitRequested` handler holds back every request
    /// except the owner's own and restarts, which Tauri cannot hold, and the
    /// final `Exit` event waits for it before the process may end.
    pub(crate) exit_settled: tokio::sync::watch::Sender<bool>,

    pub(crate) main_window_centered: AtomicBool,

    boot_progress: std::sync::Mutex<BootProgressPayload>,

    pub(crate) init_cancel: tokio_util::sync::CancellationToken,
    /// Published by the boot task; read by the exit path through
    /// `wait_for_boot_settled`.
    pub(crate) boot_phase: tokio::sync::watch::Receiver<BootPhase>,

    pub(crate) data_dir: PathBuf,
    pub(crate) log_dir: PathBuf,
    pub(crate) desktop_notifications: std::sync::Mutex<DesktopNotificationConfig>,
}

impl DesktopBackendState {
    pub(crate) fn new(
        instance_lock: Option<backend::RuntimeLease>,
        data_dir: PathBuf,
        log_dir: PathBuf,
        desktop_notifications: DesktopNotificationConfig,
        boot_phase: tokio::sync::watch::Receiver<BootPhase>,
    ) -> Self {
        Self {
            container: std::sync::Mutex::new(None),
            log_guard: std::sync::Mutex::new(None),
            _instance_lock: instance_lock,
            boot_failed: AtomicBool::new(false),
            shutdown_started: AtomicBool::new(false),
            exit_settled: tokio::sync::watch::Sender::new(false),
            main_window_centered: AtomicBool::new(false),
            boot_progress: std::sync::Mutex::new(BootProgressPayload::default()),
            init_cancel: tokio_util::sync::CancellationToken::new(),
            boot_phase,

            data_dir,
            log_dir,
            desktop_notifications: std::sync::Mutex::new(desktop_notifications),
        }
    }

    pub(crate) fn mark_boot_failed(&self) {
        self.boot_failed.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_boot_failed(&self) -> bool {
        self.boot_failed.load(Ordering::SeqCst)
    }

    pub(crate) fn set_boot_progress(&self, status: &str, progress: f32) -> BootProgressPayload {
        let payload = BootProgressPayload {
            status: status.to_string(),
            progress: progress.clamp(0.0, 1.0),
        };

        if let Ok(mut lock) = self.boot_progress.lock() {
            *lock = payload.clone();
        }

        payload
    }

    pub(crate) fn current_boot_progress(&self) -> BootProgressPayload {
        self.boot_progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    pub(crate) fn set_container(&self, container: Arc<backend::ServiceContainer>) {
        if let Ok(mut lock) = self.container.lock() {
            *lock = Some(container);
        }
    }

    pub(crate) fn set_log_guard(&self, log_guard: tracing_appender::non_blocking::WorkerGuard) {
        if let Ok(mut lock) = self.log_guard.lock() {
            *lock = Some(log_guard);
        }
    }

    pub(crate) fn backend(&self) -> Option<Arc<backend::ServiceContainer>> {
        self.container.lock().ok().and_then(|c| c.clone())
    }
}

#[tauri::command]
pub fn restart_desktop(app: tauri::AppHandle) {
    // `ExitRequestApi::prevent_exit` is ignored for restart requests, so the
    // `ExitRequested` handler cannot hold a restart back: contain first.
    if !begin_exit(&app) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        drain_backend_for_exit(&app).await;
        mark_exit_settled(&app);
        app.request_restart();
    });
}

#[tauri::command]
pub fn quit_desktop(app: tauri::AppHandle) {
    app.exit(0);
}

pub(crate) fn mark_exit_settled(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<DesktopBackendState>() {
        state.exit_settled.send_replace(true);
    }
}

/// Wait until the exit owner has drained the backend. Returns `false` when
/// `cap` elapses first.
pub(crate) async fn wait_for_exit_settled(
    mut exit_settled: tokio::sync::watch::Receiver<bool>,
    cap: Duration,
) -> bool {
    tokio::time::timeout(cap, exit_settled.wait_for(|settled| *settled))
        .await
        .is_ok()
}

/// Exit now that the backend is drained; the `ExitRequested` handler lets the
/// owner's own request through while an exit is owned.
pub(crate) fn finish_exit(app: &tauri::AppHandle, code: i32) {
    mark_exit_settled(app);
    app.exit(code);
}

/// Drain the backend on a task of its own, so that a panic inside the drain
/// still lets the owner finish the exit: while an exit is owned every other
/// request is held back, so the owner must always reach `finish_exit`.
pub(crate) async fn drain_backend_for_exit(app: &tauri::AppHandle) {
    let drain = tauri::async_runtime::spawn({
        let app = app.clone();
        async move { contain_backend_for_exit(&app).await }
    });
    if drain.await.is_err() {
        log::error!("Backend drain task ended abnormally; continuing with the exit");
    }
}

/// Whether an exit sequence has been claimed.
///
/// Window-state getters called from a runtime thread block until the main
/// thread services them, and during an AppKit termination the main thread is
/// parked in the drain; callers use this to skip such reads once an exit is
/// under way.
pub(crate) fn exit_owned(app: &tauri::AppHandle) -> bool {
    app.try_state::<DesktopBackendState>()
        .is_some_and(|state| state.shutdown_started.load(Ordering::SeqCst))
}

/// Claim the single exit sequence and stop the boot task at its next
/// checkpoint.
///
/// Returns `false` when another path already owns the exit, or when the app
/// is exiting before its state was registered; that path finishes on its own.
pub(crate) fn begin_exit(app: &tauri::AppHandle) -> bool {
    let Some(state) = app.try_state::<DesktopBackendState>() else {
        return false;
    };
    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        return false;
    }
    // Cancel before `contain_backend_for_exit` reads the boot phase; see
    // `wait_for_boot_settled` for the ordering this relies on.
    state.init_cancel.cancel();
    true
}

/// Drain the backend so no recording child outlives the process.
///
/// Waits for the boot task first: `ServiceContainer::initialize` may still be
/// spawning the very work a concurrent drain would miss.
async fn contain_backend_for_exit(app: &tauri::AppHandle) {
    let state = app.state::<DesktopBackendState>();
    if wait_for_boot_settled(state.boot_phase.clone(), BOOT_SETTLE_CAP).await
        == BootSettle::TimedOut
    {
        log::warn!(
            "Desktop boot did not settle within {:?}; continuing without waiting further (the backend is drained only if it was stored)",
            BOOT_SETTLE_CAP
        );
    }
    let Some(container) = state.backend() else {
        return;
    };
    if let Err(e) = container
        .shutdown_with_hard_cap(SHUTDOWN_GRACE_PERIOD, SHUTDOWN_HARD_CAP)
        .await
    {
        log::error!("Error during shutdown: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_settle_does_not_wait_before_services_start() {
        let (_tx, rx) = tokio::sync::watch::channel(BootPhase::Preparing);

        assert_eq!(
            wait_for_boot_settled(rx, Duration::from_secs(5)).await,
            BootSettle::NotStarted
        );
    }

    #[tokio::test]
    async fn boot_settle_waits_for_the_boot_task_to_return() {
        let (tx, rx) = tokio::sync::watch::channel(BootPhase::Services);
        let waiter = tokio::spawn(wait_for_boot_settled(rx, Duration::from_secs(5)));
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        tx.send_replace(BootPhase::Settled);

        assert_eq!(waiter.await.expect("waiter task"), BootSettle::Settled);
    }

    #[tokio::test]
    async fn boot_settle_treats_a_dropped_boot_task_as_settled() {
        let (tx, rx) = tokio::sync::watch::channel(BootPhase::Services);
        drop(tx);

        assert_eq!(
            wait_for_boot_settled(rx, Duration::from_secs(5)).await,
            BootSettle::Abandoned
        );
    }

    #[tokio::test]
    async fn boot_settle_gives_up_after_the_cap() {
        let (_tx, rx) = tokio::sync::watch::channel(BootPhase::Services);

        assert_eq!(
            wait_for_boot_settled(rx, Duration::from_millis(20)).await,
            BootSettle::TimedOut
        );
    }
}
