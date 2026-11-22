use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rust_srec::backend;
use serde::Serialize;
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::boot_error::BootProgressPayload;
use crate::desktop_notifications::DesktopNotificationConfig;
use crate::desktop_settings::{BackendMode, DesktopSettings};
use crate::tray;
use crate::window_state::{MonitorBounds, WindowState, record_window_bounds, save_window_state};

const EVENT_DESKTOP_SHUTTING_DOWN: &str = "rust-srec://desktop-shutting-down";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShuttingDownPayload {
    active_recordings: usize,
}

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
enum BootSettle {
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
async fn wait_for_boot_settled(
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
    shutdown_started: AtomicBool,
    /// Set once the exit owner has drained the backend (`finish_exit`,
    /// `restart_desktop` just before its restart, or the `Exit` arm after
    /// draining an AppKit termination). While `shutdown_started` is set and
    /// this is not, the `ExitRequested` handler holds back every request
    /// except the owner's own and restarts, which Tauri cannot hold, and the
    /// final `Exit` event waits for it before the process may end.
    pub(crate) exit_settled: tokio::sync::watch::Sender<bool>,

    pub(crate) main_window_centered: AtomicBool,
    pub(crate) window_state: Arc<std::sync::Mutex<Option<WindowState>>>,
    pub(crate) has_restored_position: AtomicBool,
    pub(crate) pending_maximize: AtomicBool,
    pub(crate) window_shown: AtomicBool,

    boot_progress: std::sync::Mutex<BootProgressPayload>,

    pub(crate) init_cancel: tokio_util::sync::CancellationToken,
    /// Published by the boot task; read by the exit path through
    /// `wait_for_boot_settled`.
    boot_phase: tokio::sync::watch::Receiver<BootPhase>,

    pub(crate) data_dir: PathBuf,
    pub(crate) log_dir: PathBuf,
    pub(crate) desktop_notifications: Arc<std::sync::Mutex<DesktopNotificationConfig>>,
    pub(crate) desktop_settings: std::sync::Mutex<DesktopSettings>,
    pub(crate) settings_write_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) window_state_write_lock: Arc<std::sync::Mutex<()>>,
    pub(crate) notifications_write_lock: Arc<std::sync::Mutex<()>>,
    pub(crate) active_backend_mode: BackendMode,
    pub(crate) active_remote_url: Option<String>,
    pub(crate) settings_notify: Arc<tokio::sync::Notify>,
    pub(crate) start_minimized_boot: AtomicBool,
    pub(crate) frontend_ready_received: AtomicBool,
    pub(crate) exit_confirm_open: AtomicBool,
    pub(crate) tray_controller: std::sync::Mutex<Option<tray::TrayController>>,
}

impl DesktopBackendState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        instance_lock: Option<backend::RuntimeLease>,
        data_dir: PathBuf,
        log_dir: PathBuf,
        desktop_notifications: DesktopNotificationConfig,
        desktop_settings: DesktopSettings,
        active_backend_mode: BackendMode,
        active_remote_url: Option<String>,
        start_minimized_boot: bool,
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
            window_state: Arc::new(std::sync::Mutex::new(None)),
            has_restored_position: AtomicBool::new(false),
            pending_maximize: AtomicBool::new(false),
            window_shown: AtomicBool::new(false),
            boot_progress: std::sync::Mutex::new(BootProgressPayload::default()),
            init_cancel: tokio_util::sync::CancellationToken::new(),
            boot_phase,

            data_dir,
            log_dir,
            desktop_notifications: Arc::new(std::sync::Mutex::new(desktop_notifications)),
            desktop_settings: std::sync::Mutex::new(desktop_settings),
            settings_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            window_state_write_lock: Arc::new(std::sync::Mutex::new(())),
            notifications_write_lock: Arc::new(std::sync::Mutex::new(())),
            active_backend_mode,
            active_remote_url,
            settings_notify: Arc::new(tokio::sync::Notify::new()),
            start_minimized_boot: AtomicBool::new(start_minimized_boot),
            frontend_ready_received: AtomicBool::new(false),
            exit_confirm_open: AtomicBool::new(false),
            tray_controller: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn mark_boot_failed(&self) {
        self.boot_failed.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_boot_failed(&self) -> bool {
        self.boot_failed.load(Ordering::SeqCst)
    }

    pub(crate) fn set_tray_controller(&self, controller: tray::TrayController) {
        if let Ok(mut lock) = self.tray_controller.lock() {
            *lock = Some(controller);
        }
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

    /// A receiver of the boot task's progress, for work that must wait until
    /// the boot task has returned.
    pub(crate) fn boot_phase(&self) -> tokio::sync::watch::Receiver<BootPhase> {
        self.boot_phase.clone()
    }

    pub(crate) fn window_state(&self) -> Option<WindowState> {
        self.window_state.lock().ok().and_then(|s| s.clone())
    }

    pub(crate) fn set_window_state(&self, window_state: WindowState) {
        if let Ok(mut lock) = self.window_state.lock() {
            *lock = Some(window_state);
        }
    }

    pub(crate) fn record_window_bounds(
        &self,
        x: Option<i32>,
        y: Option<i32>,
        width: u32,
        height: u32,
        maximized: bool,
        current_monitor: Option<MonitorBounds>,
    ) {
        if let Ok(mut lock) = self.window_state.lock() {
            record_window_bounds(&mut lock, x, y, width, height, maximized, current_monitor);
        }
    }

    /// Write the window geometry to disk on the calling thread.
    ///
    /// The geometry is read after the write lock is taken, so when two saves
    /// overlap the file ends up with the newer geometry whichever runs last.
    pub(crate) fn persist_window_state(&self) {
        let _guard = self.window_state_write_lock.lock();
        let Some(window_state) = self.window_state() else {
            return;
        };
        if let Err(e) = save_window_state(&self.data_dir, &window_state) {
            log::warn!("Failed to persist window state: {}", e);
        }
    }

    /// [`Self::persist_window_state`] on a blocking thread, for callers on the
    /// main thread.
    pub(crate) fn persist_window_state_async(&self) {
        let window_state = self.window_state.clone();
        let write_lock = self.window_state_write_lock.clone();
        let data_dir = self.data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = write_lock.lock();
            let Some(snapshot) = window_state.lock().ok().and_then(|state| state.clone()) else {
                return;
            };
            if let Err(e) = save_window_state(&data_dir, &snapshot) {
                log::warn!("Failed to persist window state: {}", e);
            }
        });
    }
}

pub(crate) fn active_recording_count(app: &tauri::AppHandle) -> usize {
    app.try_state::<DesktopBackendState>()
        .and_then(|state| state.backend())
        .map(|container| container.active_recording_summary().active_count)
        .unwrap_or(0)
}

pub(crate) fn notify_desktop_shutting_down(app: &tauri::AppHandle, active_count: usize) {
    #[cfg(desktop)]
    tray::update_tray(
        app,
        backend::ActiveRecordingSummary {
            active_count,
            total_speed_bytes_per_sec: 0,
        },
        true,
    );

    if active_count > 0
        && let Err(error) = app.emit(
            EVENT_DESKTOP_SHUTTING_DOWN,
            ShuttingDownPayload {
                active_recordings: active_count,
            },
        )
    {
        log::warn!("Failed to emit desktop shutting-down event: {}", error);
    }
}

fn proceed_with_desktop_restart(app: tauri::AppHandle) {
    // `ExitRequestApi::prevent_exit` is ignored for restart requests, so the
    // `ExitRequested` handler cannot hold a restart back: contain first.
    if !begin_exit(&app) {
        return;
    }
    let active_count = active_recording_count(&app);
    notify_desktop_shutting_down(&app, active_count);
    tauri::async_runtime::spawn(async move {
        drain_backend_for_exit(&app).await;
        mark_exit_settled(&app);
        app.request_restart();
    });
}

pub(crate) fn request_desktop_restart(app: tauri::AppHandle) {
    if exit_owned(&app) {
        return;
    }

    let Some(state) = app.try_state::<DesktopBackendState>() else {
        app.request_restart();
        return;
    };

    let active_count = active_recording_count(&app);
    let confirm = state.desktop_settings().confirm_exit_while_recording;

    if active_count > 0 && confirm {
        if state.exit_confirm_open.swap(true, Ordering::SeqCst) {
            return;
        }

        let message = if active_count == 1 {
            "1 active recording is in progress. Restarting now will stop and finalize it cleanly before restarting.".to_string()
        } else {
            format!(
                "{active_count} active recordings are in progress. Restarting now will stop and finalize them cleanly before restarting."
            )
        };

        let app_for_dialog = app.clone();
        app.dialog()
            .message(message)
            .title("Active Recordings in Progress")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Stop & Restart".to_string(),
                "Keep Recording".to_string(),
            ))
            .show(move |confirmed| {
                if let Some(state) = app_for_dialog.try_state::<DesktopBackendState>() {
                    state.exit_confirm_open.store(false, Ordering::SeqCst);
                }
                if confirmed {
                    proceed_with_desktop_restart(app_for_dialog);
                }
            });
    } else {
        proceed_with_desktop_restart(app);
    }
}

#[tauri::command]
pub fn restart_desktop(app: tauri::AppHandle) {
    request_desktop_restart(app);
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
    let app_for_save = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(state) = app_for_save.try_state::<DesktopBackendState>() {
            state.persist_window_state();
        }
    })
    .await;
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

#[tauri::command]
pub fn quit_desktop(app: tauri::AppHandle) {
    request_desktop_quit(app);
}

fn proceed_with_desktop_exit(app: &tauri::AppHandle, active_count: usize) {
    if exit_owned(app) {
        return;
    }

    notify_desktop_shutting_down(app, active_count);
    app.exit(0);
}

pub(crate) fn request_desktop_quit(app: tauri::AppHandle) {
    if exit_owned(&app) {
        return;
    }

    let Some(state) = app.try_state::<DesktopBackendState>() else {
        app.exit(0);
        return;
    };

    let active_count = state
        .backend()
        .map(|container| container.active_recording_summary().active_count)
        .unwrap_or(0);
    let confirm = state.desktop_settings().confirm_exit_while_recording;

    if active_count > 0 && confirm {
        if state.exit_confirm_open.swap(true, Ordering::SeqCst) {
            return;
        }

        let message = if active_count == 1 {
            "1 active recording is in progress. Quitting now will stop and finalize it cleanly before exiting.".to_string()
        } else {
            format!(
                "{active_count} active recordings are in progress. Quitting now will stop and finalize them cleanly before exiting."
            )
        };

        let app_for_dialog = app.clone();
        app.dialog()
            .message(message)
            .title("Active Recordings in Progress")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Stop & Quit".to_string(),
                "Keep Recording".to_string(),
            ))
            .show(move |confirmed| {
                if let Some(state) = app_for_dialog.try_state::<DesktopBackendState>() {
                    state.exit_confirm_open.store(false, Ordering::SeqCst);
                }
                if confirmed {
                    let current_active = app_for_dialog
                        .try_state::<DesktopBackendState>()
                        .and_then(|state| state.backend())
                        .map(|container| container.active_recording_summary().active_count)
                        .unwrap_or(0);
                    proceed_with_desktop_exit(&app_for_dialog, current_active);
                }
            });
    } else {
        proceed_with_desktop_exit(&app, active_count);
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
