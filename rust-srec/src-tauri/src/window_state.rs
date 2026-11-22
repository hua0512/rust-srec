use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
#[cfg(all(desktop, target_os = "macos"))]
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::json_file::write_json_atomically;
use crate::lifecycle::{DesktopBackendState, exit_owned, request_desktop_quit};

pub const DEFAULT_WINDOW_WIDTH: u32 = 1280;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 1024;
pub const MIN_WINDOW_WIDTH: u32 = 800;
pub const MIN_WINDOW_HEIGHT: u32 = 600;

/// Minimum horizontal overlap (in physical pixels) required on a monitor to
/// consider a saved window position reachable.
const MIN_VISIBLE_WIDTH: u32 = 160;

/// Minimum vertical overlap (in physical pixels) required on a monitor to
/// consider a saved window position reachable.
const MIN_VISIBLE_HEIGHT: u32 = 80;

/// Small allowance outside a monitor's edges (in physical pixels) to account
/// for platform outer-frame shadows/borders (e.g. Windows DWM invisible borders
/// when snapped to a screen edge) while still rejecting off-screen windows.
const EDGE_OVERHANG_MARGIN: i64 = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn window_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("window_state.json")
}

pub fn load_window_state(data_dir: &Path) -> io::Result<Option<WindowState>> {
    let path = window_state_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let state = serde_json::from_str::<WindowState>(&raw)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_window_state(data_dir: &Path, state: &WindowState) -> io::Result<()> {
    write_json_atomically(&window_state_path(data_dir), state)
}

fn monitor_contains_point(monitor: MonitorBounds, x: i32, y: i32) -> bool {
    let left = i64::from(monitor.x) - EDGE_OVERHANG_MARGIN;
    let top = i64::from(monitor.y) - EDGE_OVERHANG_MARGIN;
    let right = i64::from(monitor.x) + i64::from(monitor.width) + EDGE_OVERHANG_MARGIN;
    let bottom = i64::from(monitor.y) + i64::from(monitor.height) + EDGE_OVERHANG_MARGIN;
    let px = i64::from(x);
    let py = i64::from(y);
    px >= left && px < right && py >= top && py < bottom
}

/// Validate a persisted window geometry against the currently connected monitors.
///
/// Returns `None` when the saved size is empty or no monitor provides a
/// reachable top edge and meaningful visible overlap, signaling the caller to
/// fall back to the default centered geometry. When `x`/`y` are absent (e.g.
/// Wayland, where outer coordinates cannot be queried), size and maximize state
/// are still restored and clamped against the available display.
pub fn resolve_restored_window_state(
    saved: &WindowState,
    monitors: &[MonitorBounds],
    min_width: u32,
    min_height: u32,
) -> Option<WindowState> {
    if saved.width == 0 || saved.height == 0 || monitors.is_empty() {
        return None;
    }

    let (Some(saved_x), Some(saved_y)) = (saved.x, saved.y) else {
        let reference = monitors
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .max_by_key(|m| u64::from(m.width) * u64::from(m.height))?;
        let effective_min_w = min_width.min(reference.width);
        let effective_min_h = min_height.min(reference.height);
        return Some(WindowState {
            x: None,
            y: None,
            width: saved.width.clamp(effective_min_w, reference.width),
            height: saved.height.clamp(effective_min_h, reference.height),
            maximized: saved.maximized,
        });
    };

    let win_left = i64::from(saved_x);
    let win_top = i64::from(saved_y);
    let win_right = win_left + i64::from(saved.width);
    let win_bottom = win_top + i64::from(saved.height);

    let required_w = i64::from(saved.width.min(MIN_VISIBLE_WIDTH));
    let required_h = i64::from(saved.height.min(MIN_VISIBLE_HEIGHT));

    let best_monitor = monitors
        .iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .filter_map(|m| {
            let mon_left = i64::from(m.x);
            let mon_top = i64::from(m.y);
            let mon_right = mon_left + i64::from(m.width);
            let mon_bottom = mon_top + i64::from(m.height);

            // The title bar must remain reachable within the monitor's vertical span.
            if win_top < mon_top - EDGE_OVERHANG_MARGIN || win_top >= mon_bottom {
                return None;
            }

            let overlap_w = (win_right.min(mon_right) - win_left.max(mon_left)).max(0);
            let overlap_h = (win_bottom.min(mon_bottom) - win_top.max(mon_top)).max(0);
            if overlap_w < required_w || overlap_h < required_h {
                return None;
            }

            Some((overlap_w * overlap_h, *m))
        })
        .max_by_key(|(area, _)| *area)
        .map(|(_, monitor)| monitor)?;

    let effective_min_w = min_width.min(best_monitor.width);
    let effective_min_h = min_height.min(best_monitor.height);
    let width = saved.width.clamp(effective_min_w, best_monitor.width);
    let height = saved.height.clamp(effective_min_h, best_monitor.height);

    let mon_left = i64::from(best_monitor.x);
    let mon_top = i64::from(best_monitor.y);
    let mon_right = mon_left + i64::from(best_monitor.width);
    let mon_bottom = mon_top + i64::from(best_monitor.height);
    let max_x = mon_left + i64::from(best_monitor.width - width);
    let max_y = mon_top + i64::from(best_monitor.height - height);

    // Preserve small DWM invisible-border overhangs (e.g. (-8, 0) when snapped
    // on Windows) instead of shifting the window away from screen edges.
    let clamped_x = if win_left >= mon_left - EDGE_OVERHANG_MARGIN
        && win_left < mon_left
        && win_left + i64::from(width) <= mon_right + EDGE_OVERHANG_MARGIN
    {
        win_left
    } else {
        win_left.clamp(mon_left, max_x)
    };
    let clamped_y = if win_top >= mon_top - EDGE_OVERHANG_MARGIN
        && win_top < mon_top
        && win_top + i64::from(height) <= mon_bottom + EDGE_OVERHANG_MARGIN
    {
        win_top
    } else {
        win_top.clamp(mon_top, max_y)
    };

    let x = clamped_x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let y = clamped_y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;

    Some(WindowState {
        x: Some(x),
        y: Some(y),
        width,
        height,
        maximized: saved.maximized,
    })
}

/// Update the in-memory tracked window state from a live window snapshot.
///
/// When `maximized` is true, preserves previously recorded unmaximized size and
/// position unless the maximized window was moved onto a different monitor, in
/// which case the anchor position is re-centered on `current_monitor` so the
/// next launch maximizes on the new display.
pub fn record_window_bounds(
    current: &mut Option<WindowState>,
    x: Option<i32>,
    y: Option<i32>,
    width: u32,
    height: u32,
    maximized: bool,
    current_monitor: Option<MonitorBounds>,
) {
    if maximized {
        if let Some(state) = current.as_mut() {
            state.maximized = true;
            if let Some(mon) = current_monitor {
                let on_same_monitor = state
                    .x
                    .zip(state.y)
                    .is_some_and(|(sx, sy)| monitor_contains_point(mon, sx, sy));
                if !on_same_monitor {
                    let clamped_w = state.width.min(mon.width);
                    let clamped_h = state.height.min(mon.height);
                    let new_x = i64::from(mon.x) + i64::from(mon.width - clamped_w) / 2;
                    let new_y = i64::from(mon.y) + i64::from(mon.height - clamped_h) / 2;
                    state.x = Some(new_x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                    state.y = Some(new_y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                }
            }
        } else if width > 0 && height > 0 {
            *current = Some(WindowState {
                x,
                y,
                width,
                height,
                maximized: true,
            });
        }
        return;
    }

    if width == 0 || height == 0 {
        return;
    }

    *current = Some(WindowState {
        x,
        y,
        width,
        height,
        maximized: false,
    });
}

fn current_monitor_bounds(window: &tauri::WebviewWindow) -> Option<MonitorBounds> {
    window.current_monitor().ok().flatten().map(|monitor| {
        let pos = monitor.position();
        let size = monitor.size();
        MonitorBounds {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
        }
    })
}

fn capture_main_window_bounds(window: &tauri::WebviewWindow, state: &DesktopBackendState) {
    if window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(false) {
        return;
    }

    let pos = window.outer_position().ok().map(|p| (p.x, p.y));
    let (x, y) = match pos {
        Some((x, y)) => (Some(x), Some(y)),
        None => (None, None),
    };
    let monitor = current_monitor_bounds(window);

    if window.is_maximized().unwrap_or(false) {
        let (width, height) = window
            .inner_size()
            .map(|s| (s.width, s.height))
            .unwrap_or((0, 0));
        state.record_window_bounds(x, y, width, height, true, monitor);
        return;
    }

    if let Ok(size) = window.inner_size() {
        state.record_window_bounds(x, y, size.width, size.height, false, monitor);
    }
}

fn track_main_window_geometry(app: &tauri::AppHandle) {
    if exit_owned(app) {
        return;
    }
    let Some(state) = app.try_state::<DesktopBackendState>() else {
        return;
    };
    if !state.window_shown.load(Ordering::SeqCst) {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    capture_main_window_bounds(&window, &state);
}

pub(crate) fn restore_main_window_state(
    window: &tauri::WebviewWindow,
    state: &DesktopBackendState,
) {
    let saved = match load_window_state(&state.data_dir) {
        Ok(Some(saved)) => saved,
        Ok(None) => return,
        Err(e) => {
            log::warn!("Failed to load saved window state: {}", e);
            return;
        }
    };

    let monitors: Vec<MonitorBounds> = match window.available_monitors() {
        Ok(list) => list
            .into_iter()
            .map(|monitor| {
                let pos = monitor.position();
                let size = monitor.size();
                MonitorBounds {
                    x: pos.x,
                    y: pos.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect(),
        Err(e) => {
            log::warn!(
                "Failed to query available monitors for window restore: {}",
                e
            );
            return;
        }
    };

    let Some(resolved) =
        resolve_restored_window_state(&saved, &monitors, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
    else {
        return;
    };

    // Position first: moving a window to a monitor with another scale factor
    // rescales it, so the saved size is applied once it is on that monitor.
    let pos_ok = match (resolved.x, resolved.y) {
        (Some(x), Some(y)) => match window.set_position(tauri::PhysicalPosition::new(x, y)) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("Failed to restore main window position: {}", e);
                false
            }
        },
        _ => false,
    };
    let size_ok = match window.set_size(tauri::PhysicalSize::new(resolved.width, resolved.height)) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("Failed to restore main window size: {}", e);
            false
        }
    };

    if size_ok {
        let should_maximize = resolved.maximized;
        state.set_window_state(resolved);
        if pos_ok {
            state.has_restored_position.store(true, Ordering::SeqCst);
        }
        if should_maximize {
            state.pending_maximize.store(true, Ordering::SeqCst);
        }
    }
}

/// A page of the desktop frontend: served by the Vite dev server under
/// `tauri dev`, bundled with the app otherwise.
pub(crate) fn desktop_page_url(page: &str) -> tauri::WebviewUrl {
    if tauri::is_dev()
        && let Ok(url) = tauri::Url::parse(&format!("http://127.0.0.1:15275/{page}"))
    {
        tauri::WebviewUrl::External(url)
    } else {
        tauri::WebviewUrl::App(page.into())
    }
}

/// Handle the user clicking the main window's close button (`X`).
///
/// A recovery window has no running backend, so closing it exits the process
/// instead of stranding the database `RuntimeLease` in a headless tray
/// instance. Any other window hides to the tray or quits the app, as the
/// close-to-tray setting says.
pub(crate) fn on_main_window_close_requested(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<DesktopBackendState>() else {
        hide_main_window(app);
        return;
    };
    if state.is_boot_failed() {
        app.exit(0);
    } else if state.desktop_settings().close_to_tray {
        hide_main_window(app);
    } else {
        request_desktop_quit(app.clone());
    }
}

pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<DesktopBackendState>() {
        state.start_minimized_boot.store(false, Ordering::SeqCst);
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    center_main_window_once(app);
    let _ = window.show();
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    if let Some(state) = app.try_state::<DesktopBackendState>() {
        if state.pending_maximize.swap(false, Ordering::SeqCst) {
            let _ = window.maximize();
        } else if state.window_state().is_none() {
            capture_main_window_bounds(&window, &state);
        }
        state.window_shown.store(true, Ordering::SeqCst);
    }
    let _ = window.set_focus();
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<DesktopBackendState>() {
        state.persist_window_state_async();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

pub(crate) fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        // If the user clicked the tray while starting minimized before the main
        // window is created, clear the minimized boot flag so it shows when ready.
        if let Some(state) = app.try_state::<DesktopBackendState>() {
            state.start_minimized_boot.store(false, Ordering::SeqCst);
        }
        return;
    };
    match window.is_visible() {
        Ok(true) => hide_main_window(app),
        Ok(false) => show_main_window(app),
        Err(_) => show_main_window(app),
    }
}

pub(crate) fn center_main_window_once(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let should_center = app
        .try_state::<DesktopBackendState>()
        .map(|state| {
            if state.has_restored_position.load(Ordering::SeqCst) {
                return false;
            }
            !state.main_window_centered.swap(true, Ordering::SeqCst)
        })
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
fn hide_main_window_if_minimized(app: &tauri::AppHandle) -> bool {
    if exit_owned(app) {
        return false;
    }
    if let Some(state) = app.try_state::<DesktopBackendState>()
        && (state.is_boot_failed() || !state.desktop_settings().minimize_to_tray)
    {
        return false;
    }
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    if !window.is_minimized().unwrap_or(false) {
        return false;
    }
    if let Some(state) = app.try_state::<DesktopBackendState>() {
        state.persist_window_state_async();
    }
    if let Err(error) = window.hide() {
        log::warn!("Failed to hide minimized window: {}", error);
    }
    true
}

#[cfg(desktop)]
pub(crate) fn handle_main_window_event(app: &tauri::AppHandle, event: &tauri::WindowEvent) {
    if matches!(
        event,
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
    ) {
        track_main_window_geometry(app);
    }
    handle_main_window_minimize_event(app, event);
}

/// Move a minimized main window to the tray.
///
/// Tao reports minimization through `Resized`: WM_SIZE on Windows and the GTK
/// window-state change on Linux, both delivered after the minimized state is
/// updated, so one read suffices.
#[cfg(all(desktop, not(target_os = "macos")))]
fn handle_main_window_minimize_event(app: &tauri::AppHandle, event: &tauri::WindowEvent) {
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
fn handle_main_window_minimize_event(app: &tauri::AppHandle, event: &tauri::WindowEvent) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_valid_position_on_connected_monitor() {
        let monitors = [
            MonitorBounds {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorBounds {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ];
        let saved = WindowState {
            x: Some(-1700),
            y: Some(120),
            width: 1280,
            height: 900,
            maximized: true,
        };

        let resolved =
            resolve_restored_window_state(&saved, &monitors, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        assert_eq!(resolved, Some(saved));

        // Windows DWM invisible border overhang (-8, 0) is preserved without shifting.
        let snapped = WindowState {
            x: Some(-8),
            y: Some(0),
            width: 1280,
            height: 1040,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(&snapped, &monitors, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT),
            Some(snapped)
        );

        // Wayland-style saved geometry without coordinates restores clamped size & maximize.
        let positionless = WindowState {
            x: None,
            y: None,
            width: 1280,
            height: 900,
            maximized: true,
        };
        assert_eq!(
            resolve_restored_window_state(
                &positionless,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            Some(positionless)
        );
    }

    #[test]
    fn returns_none_for_disconnected_or_offscreen_monitor() {
        let monitors = [MonitorBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let on_removed_display = WindowState {
            x: Some(2100),
            y: Some(100),
            width: 1280,
            height: 900,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(
                &on_removed_display,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            None
        );

        let barely_peeking = WindowState {
            x: Some(1850),
            y: Some(100),
            width: 1280,
            height: 900,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(
                &barely_peeking,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            None
        );

        let zero_sized = WindowState {
            x: Some(100),
            y: Some(100),
            width: 0,
            height: 800,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(
                &zero_sized,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            None
        );
    }

    #[test]
    fn returns_none_when_top_edge_is_unreachable() {
        let monitors = [MonitorBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];

        let titlebar_above_screen = WindowState {
            x: Some(200),
            y: Some(-120),
            width: 1280,
            height: 900,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(
                &titlebar_above_screen,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            None
        );

        let titlebar_below_bottom = WindowState {
            x: Some(200),
            y: Some(1040),
            width: 1280,
            height: 900,
            maximized: false,
        };
        assert_eq!(
            resolve_restored_window_state(
                &titlebar_below_bottom,
                &monitors,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            ),
            None
        );
    }

    #[test]
    fn clamps_oversized_window_to_monitor_bounds() {
        let monitors = [MonitorBounds {
            x: 0,
            y: 0,
            width: 1366,
            height: 768,
        }];
        let saved = WindowState {
            x: Some(200),
            y: Some(100),
            width: 2560,
            height: 1440,
            maximized: false,
        };

        let resolved =
            resolve_restored_window_state(&saved, &monitors, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        assert_eq!(
            resolved,
            Some(WindowState {
                x: Some(0),
                y: Some(0),
                width: 1366,
                height: 768,
                maximized: false,
            })
        );
    }

    #[test]
    fn maximized_update_preserves_normal_bounds_and_tracks_monitor_moves() {
        let primary = MonitorBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let secondary = MonitorBounds {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        };

        let mut tracked = None;
        record_window_bounds(
            &mut tracked,
            Some(140),
            Some(90),
            1280,
            900,
            false,
            Some(primary),
        );
        assert_eq!(
            tracked,
            Some(WindowState {
                x: Some(140),
                y: Some(90),
                width: 1280,
                height: 900,
                maximized: false,
            })
        );

        // Maximizing on the same monitor preserves the saved unmaximized bounds.
        record_window_bounds(
            &mut tracked,
            Some(-8),
            Some(-8),
            1920,
            1040,
            true,
            Some(primary),
        );
        assert_eq!(
            tracked,
            Some(WindowState {
                x: Some(140),
                y: Some(90),
                width: 1280,
                height: 900,
                maximized: true,
            })
        );

        // Moving the maximized window to a second monitor re-anchors to that
        // monitor while preserving the normal width/height.
        record_window_bounds(
            &mut tracked,
            Some(1912),
            Some(-8),
            2560,
            1400,
            true,
            Some(secondary),
        );
        assert_eq!(
            tracked,
            Some(WindowState {
                x: Some(1920 + (2560 - 1280) / 2),
                y: Some((1440 - 900) / 2),
                width: 1280,
                height: 900,
                maximized: true,
            })
        );

        // Zero-sized normal snapshots are ignored.
        record_window_bounds(&mut tracked, Some(-32000), Some(-32000), 0, 0, false, None);
        assert_eq!(
            tracked.as_ref().map(|s| (s.width, s.height, s.maximized)),
            Some((1280, 900, true))
        );

        // Restoring to a normal window updates bounds and clears `maximized`.
        record_window_bounds(
            &mut tracked,
            Some(200),
            Some(110),
            1320,
            860,
            false,
            Some(primary),
        );
        assert_eq!(
            tracked,
            Some(WindowState {
                x: Some(200),
                y: Some(110),
                width: 1320,
                height: 860,
                maximized: false,
            })
        );
    }
}
