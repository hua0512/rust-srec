use std::sync::atomic::Ordering;
#[cfg(all(desktop, target_os = "macos"))]
use std::time::Duration;

use tauri::Manager;

use crate::DesktopBackendState;
use crate::lifecycle::exit_owned;

/// Handle the user clicking the main window's close button (`X`).
///
/// Normal windows hide to the system tray while the embedded backend continues
/// running. A boot-error recovery window has no running backend, so closing it
/// exits the process instead of stranding the database `RuntimeLease` in a
/// headless tray instance.
pub(crate) fn on_main_window_close_requested(app_handle: &tauri::AppHandle) {
    let boot_failed = app_handle
        .try_state::<DesktopBackendState>()
        .is_some_and(|state| state.is_boot_failed());
    if boot_failed {
        app_handle.exit(0);
    } else {
        hide_main_window(app_handle);
    }
}

pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

pub(crate) fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[cfg(desktop)]
pub(crate) fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => hide_main_window(app),
        Ok(false) | Err(_) => show_main_window(app),
    }
}

pub(crate) fn center_main_window_once(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let should_center = app
        .try_state::<DesktopBackendState>()
        .map(|state| !state.main_window_centered.swap(true, Ordering::SeqCst))
        .unwrap_or(true);

    if !should_center {
        return;
    }

    if let Err(e) = window.center() {
        log::warn!("Failed to center main window: {}", e);
    }
}

/// Hide the main window once the platform reports it minimized, so it lives in
/// the tray rather than the taskbar or Dock. Returns whether it was minimized.
#[cfg(desktop)]
fn hide_main_window_if_minimized(app: &tauri::AppHandle) -> bool {
    if exit_owned(app)
        || app
            .try_state::<DesktopBackendState>()
            .is_some_and(|state| state.is_boot_failed())
    {
        return false;
    }
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    if !window.is_minimized().unwrap_or(false) {
        return false;
    }
    if let Err(error) = window.hide() {
        log::warn!("Failed to hide minimized window: {}", error);
    }
    true
}

/// Move a minimized main window to the tray.
///
/// Tao reports minimization through `Resized`: WM_SIZE on Windows and the GTK
/// window-state change on Linux, both delivered after the minimized state is
/// updated, so one read suffices.
#[cfg(all(desktop, not(target_os = "macos")))]
pub(crate) fn handle_main_window_event(app: &tauri::AppHandle, event: &tauri::WindowEvent) {
    if matches!(event, tauri::WindowEvent::Resized(_)) {
        hide_main_window_if_minimized(app);
    }
}

/// Successive delays between re-reads of the minimized state after the window
/// resigns key status. AppKit resigns key while the miniaturize animation is
/// still running, so the read that follows the event can precede the state
/// change; the default animation finishes well inside the first two re-reads
/// and the last one is a margin for a slowed animation.
#[cfg(all(desktop, target_os = "macos"))]
const MINIMIZE_RECHECK_DELAYS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(1000),
    Duration::from_millis(3000),
];

/// Move a minimized main window to the tray.
///
/// macOS has no minimize event; the window only resigns key status, which
/// also happens on every app switch. A miss is re-read a little later rather
/// than polled for. Minimizing a window that is not key (from the Dock, or the
/// yellow button of a background window) resigns nothing, so that window stays
/// in the Dock until it is restored from the tray.
#[cfg(all(desktop, target_os = "macos"))]
pub(crate) fn handle_main_window_event(app: &tauri::AppHandle, event: &tauri::WindowEvent) {
    if !matches!(event, tauri::WindowEvent::Focused(false)) || hide_main_window_if_minimized(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for delay in MINIMIZE_RECHECK_DELAYS {
            tokio::time::sleep(delay).await;
            if hide_main_window_if_minimized(&app) {
                break;
            }
        }
    });
}
