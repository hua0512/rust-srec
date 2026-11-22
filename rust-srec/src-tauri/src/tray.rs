use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rust_srec::backend::ActiveRecordingSummary;
use tauri::Emitter;
use tauri::Manager;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri_plugin_opener::OpenerExt;

use crate::desktop_notifications::show_desktop_notification;
use crate::desktop_settings::open_recordings_folder_for_app;
use crate::lifecycle::{DesktopBackendState, exit_owned, request_desktop_quit};
use crate::updates::{EVENT_DESKTOP_UPDATE_AVAILABLE, fetch_update_info};
use crate::window_state::{show_main_window, toggle_main_window};

pub const TRAY_ID: &str = "rust-srec-tray";

/// Longest gap between two left clicks on the tray icon that are handled as
/// one double-click. Equals the default Windows double-click time.
const TRAY_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

pub fn format_speed_bytes_per_sec(bytes_per_sec: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    if bytes_per_sec < 1024 {
        format!("{bytes_per_sec} B/s")
    } else if bytes_per_sec < 1024 * 1024 {
        let kb = bytes_per_sec as f64 / KIB;
        if kb >= 10.0 {
            format!("{kb:.0} KB/s")
        } else {
            format!("{kb:.1} KB/s")
        }
    } else if bytes_per_sec < 1024 * 1024 * 1024 {
        let mb = bytes_per_sec as f64 / MIB;
        format!("{mb:.1} MB/s")
    } else {
        let gb = bytes_per_sec as f64 / GIB;
        format!("{gb:.2} GB/s")
    }
}

pub fn format_tray_tooltip(summary: ActiveRecordingSummary, shutting_down: bool) -> String {
    if shutting_down {
        return if summary.active_count > 0 {
            "Rust-Srec — Finalizing recordings before exit...".to_string()
        } else {
            "Rust-Srec — Exiting...".to_string()
        };
    }

    match summary.active_count {
        0 => "Rust-Srec — Idle".to_string(),
        1 => {
            if summary.total_speed_bytes_per_sec == 0 {
                "Rust-Srec — Recording 1 stream".to_string()
            } else {
                format!(
                    "Rust-Srec — Recording 1 stream ({})",
                    format_speed_bytes_per_sec(summary.total_speed_bytes_per_sec)
                )
            }
        }
        count => {
            if summary.total_speed_bytes_per_sec == 0 {
                format!("Rust-Srec — Recording {count} streams")
            } else {
                format!(
                    "Rust-Srec — Recording {count} streams ({})",
                    format_speed_bytes_per_sec(summary.total_speed_bytes_per_sec)
                )
            }
        }
    }
}

pub fn format_tray_status_label(summary: ActiveRecordingSummary, shutting_down: bool) -> String {
    if shutting_down {
        return "Finalizing recordings...".to_string();
    }

    match summary.active_count {
        0 => "Idle — No active recordings".to_string(),
        1 => {
            if summary.total_speed_bytes_per_sec == 0 {
                "Recording: 1 stream".to_string()
            } else {
                format!(
                    "Recording: 1 stream ({})",
                    format_speed_bytes_per_sec(summary.total_speed_bytes_per_sec)
                )
            }
        }
        count => {
            if summary.total_speed_bytes_per_sec == 0 {
                format!("Recording: {count} streams")
            } else {
                format!(
                    "Recording: {count} streams ({})",
                    format_speed_bytes_per_sec(summary.total_speed_bytes_per_sec)
                )
            }
        }
    }
}

fn blend_rgba(dst: &mut [u8], src: [u8; 4], coverage: f32) {
    let src_a = (f32::from(src[3]) / 255.0) * coverage.clamp(0.0, 1.0);
    if src_a <= 0.0 {
        return;
    }
    let dst_a = f32::from(dst[3]) / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }

    for channel in 0..3 {
        let s = f32::from(src[channel]);
        let d = f32::from(dst[channel]);
        let blended = (s * src_a + d * dst_a * (1.0 - src_a)) / out_a;
        dst[channel] = blended.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Overlay a red recording indicator dot with a crisp white border ring in the
/// bottom-right corner of an RGBA tray icon buffer.
pub fn composite_recording_badge(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let expected_len = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || out.len() < expected_len {
        return out;
    }

    const RED: [u8; 4] = [239, 68, 68, 255];
    const RING: [u8; 4] = [255, 255, 255, 240];

    let min_dim = width.min(height) as f32;
    let max_r = (min_dim * 0.45).max(0.5);
    let outer_r = (min_dim * 0.19).clamp(2.5_f32.min(max_r), max_r);
    let inner_r = (outer_r * 0.72).max(1.5_f32.min(outer_r));
    let margin = (min_dim * 0.03).max(0.5);
    let cx = width as f32 - outer_r - margin;
    let cy = height as f32 - outer_r - margin;

    let min_x = (cx - outer_r - 1.0).floor().max(0.0) as u32;
    let max_x = (cx + outer_r + 1.0).ceil().min(width as f32) as u32;
    let min_y = (cy - outer_r - 1.0).floor().max(0.0) as u32;
    let max_y = (cy + outer_r + 1.0).ceil().min(height as f32) as u32;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = (x as f32 + 0.5) - cx;
            let dy = (y as f32 + 0.5) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > outer_r + 0.5 {
                continue;
            }

            let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
            let pixel = &mut out[idx..idx + 4];

            let ring_cov = (outer_r + 0.5 - dist).clamp(0.0, 1.0);
            blend_rgba(pixel, RING, ring_cov);

            if dist < inner_r + 0.5 {
                let red_cov = (inner_r + 0.5 - dist).clamp(0.0, 1.0);
                blend_rgba(pixel, RED, red_cov);
            }
        }
    }

    out
}

pub(crate) struct TrayController {
    status_item: MenuItem<tauri::Wry>,
    idle_icon: Image<'static>,
    recording_icon: Image<'static>,
    remote_host: Option<String>,
    last_recording: bool,
    last_tooltip: String,
    last_status_label: String,
    shutting_down: bool,
}

struct PlannedTrayUpdate {
    status_item: Option<(MenuItem<tauri::Wry>, String)>,
    tooltip: Option<String>,
    icon: Option<Image<'static>>,
}

impl TrayController {
    fn plan_update(
        &mut self,
        summary: ActiveRecordingSummary,
        shutting_down: bool,
    ) -> Option<PlannedTrayUpdate> {
        if self.shutting_down && !shutting_down {
            return None;
        }
        if shutting_down {
            self.shutting_down = true;
        }

        let (recording, tooltip, status_label) = match self.remote_host.as_deref() {
            Some(host) => (
                false,
                if shutting_down {
                    "Rust-Srec — Exiting...".to_string()
                } else {
                    format!("Rust-Srec — Remote ({host})")
                },
                if shutting_down {
                    "Exiting...".to_string()
                } else {
                    format!("Remote: {host}")
                },
            ),
            None => (
                summary.active_count > 0,
                format_tray_tooltip(summary, shutting_down),
                format_tray_status_label(summary, shutting_down),
            ),
        };

        let icon = if recording != self.last_recording {
            self.last_recording = recording;
            Some(if recording {
                self.recording_icon.clone()
            } else {
                self.idle_icon.clone()
            })
        } else {
            None
        };

        let tooltip = if tooltip != self.last_tooltip {
            self.last_tooltip = tooltip.clone();
            Some(tooltip)
        } else {
            None
        };

        let status_item = if status_label != self.last_status_label {
            self.last_status_label = status_label.clone();
            Some((self.status_item.clone(), status_label))
        } else {
            None
        };

        if icon.is_none() && tooltip.is_none() && status_item.is_none() {
            return None;
        }

        Some(PlannedTrayUpdate {
            status_item,
            tooltip,
            icon,
        })
    }
}

pub(crate) fn setup_tray(
    app: &tauri::AppHandle,
    remote_host: Option<String>,
) -> tauri::Result<TrayController> {
    let initial_summary = ActiveRecordingSummary {
        active_count: 0,
        total_speed_bytes_per_sec: 0,
    };
    let (initial_tooltip, initial_status_label) = match remote_host.as_deref() {
        Some(host) => (
            format!("Rust-Srec — Remote ({host})"),
            format!("Remote: {host}"),
        ),
        None => (
            format_tray_tooltip(initial_summary, false),
            format_tray_status_label(initial_summary, false),
        ),
    };

    let status_item = MenuItem::new(app, &initial_status_label, false, None::<&str>)?;
    let sep_top = PredefinedMenuItem::separator(app)?;
    let show_hide = MenuItem::new(app, "Show / Hide", true, None::<&str>)?;
    let open_recordings = remote_host
        .is_none()
        .then(|| MenuItem::new(app, "Open Recordings Folder", true, None::<&str>))
        .transpose()?;
    let open_logs = MenuItem::new(app, "Open Logs Folder", true, None::<&str>)?;
    let open_data = MenuItem::new(app, "Open Data Folder", true, None::<&str>)?;
    let check_updates = MenuItem::new(app, "Check for Updates", true, None::<&str>)?;
    let sep_bottom = PredefinedMenuItem::separator(app)?;

    #[cfg(feature = "devtools")]
    let devtools = {
        let enabled =
            cfg!(debug_assertions) || std::env::var_os("RUST_SREC_DESKTOP_DEVTOOLS").is_some();
        enabled
            .then(|| MenuItem::new(app, "DevTools", true, None::<&str>))
            .transpose()?
    };

    let quit = MenuItem::new(app, "Quit", true, None::<&str>)?;

    let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        vec![&status_item, &sep_top, &show_hide];

    if let Some(open_recordings_item) = &open_recordings {
        items.push(open_recordings_item);
    }

    items.push(&open_logs);
    items.push(&open_data);
    items.push(&check_updates);
    items.push(&sep_bottom);

    #[cfg(feature = "devtools")]
    if let Some(devtools_item) = &devtools {
        items.push(devtools_item);
    }

    items.push(&quit);

    let menu = Menu::with_items(app, &items)?;

    let show_hide_id = show_hide.id().clone();
    let open_recordings_id = open_recordings.as_ref().map(|item| item.id().clone());
    let open_logs_id = open_logs.id().clone();
    let open_data_id = open_data.id().clone();
    let check_updates_id = check_updates.id().clone();
    #[cfg(feature = "devtools")]
    let devtools_id = devtools.as_ref().map(|item| item.id().clone());
    let quit_id = quit.id().clone();

    let idle_icon = tauri::include_image!("./icons/32x32.png").to_owned();
    let recording_rgba =
        composite_recording_badge(idle_icon.rgba(), idle_icon.width(), idle_icon.height());
    let recording_icon = Image::new_owned(recording_rgba, idle_icon.width(), idle_icon.height());

    let last_left_click = Mutex::new(None::<Instant>);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(idle_icon.clone())
        .tooltip(&initial_tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app: &tauri::AppHandle, event| {
            if event.id == quit_id {
                request_desktop_quit(app.clone());
                return;
            }
            #[cfg(feature = "devtools")]
            {
                if devtools_id.as_ref().is_some_and(|id| event.id == *id) {
                    if let Some(window) = app.get_webview_window("main") {
                        if window.is_devtools_open() {
                            window.close_devtools();
                        } else {
                            window.open_devtools();
                        }
                    }
                    return;
                }
            }
            if event.id == show_hide_id {
                toggle_main_window(app);
                return;
            }
            if open_recordings_id
                .as_ref()
                .is_some_and(|id| event.id == *id)
            {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let Some(state) = app_handle.try_state::<DesktopBackendState>() else {
                        return;
                    };
                    if let Err(error) = open_recordings_folder_for_app(&app_handle, &state).await {
                        log::warn!("Failed to open recordings folder from tray: {}", error);
                    }
                });
                return;
            }
            if event.id == open_logs_id {
                if let Some(state) = app.try_state::<DesktopBackendState>() {
                    let path = state.log_dir.to_string_lossy().into_owned();
                    if let Err(error) = app.opener().open_path(path.clone(), None::<&str>) {
                        log::warn!("Failed to open logs folder '{}' from tray: {}", path, error);
                    }
                }
                return;
            }
            if event.id == open_data_id {
                if let Some(state) = app.try_state::<DesktopBackendState>() {
                    let path = state.data_dir.to_string_lossy().into_owned();
                    if let Err(error) = app.opener().open_path(path.clone(), None::<&str>) {
                        log::warn!("Failed to open data folder '{}' from tray: {}", path, error);
                    }
                }
                return;
            }
            if event.id == check_updates_id {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    match fetch_update_info().await {
                        Ok(info) if info.update_available => {
                            if let Err(error) =
                                app_handle.emit(EVENT_DESKTOP_UPDATE_AVAILABLE, &info)
                            {
                                log::warn!(
                                    "Failed to emit desktop update available event: {}",
                                    error
                                );
                            }
                            let main_visible = app_handle
                                .get_webview_window("main")
                                .and_then(|window| window.is_visible().ok())
                                .unwrap_or(false);
                            if main_visible {
                                show_main_window(&app_handle);
                            } else if let Err(error) = show_desktop_notification(
                                &app_handle,
                                "Rust-Srec Update Available".to_string(),
                                format!(
                                    "Version v{} is available (current: v{}).",
                                    info.latest_version, info.current_version
                                ),
                            )
                            .await
                            {
                                log::warn!(
                                    "Failed to show update available notification: {}",
                                    error
                                );
                            }
                        }
                        Ok(info) => {
                            if let Err(error) = show_desktop_notification(
                                &app_handle,
                                "Rust-Srec".to_string(),
                                format!("Rust-Srec is up to date (v{})", info.current_version),
                            )
                            .await
                            {
                                log::warn!("Failed to show up-to-date notification: {}", error);
                            }
                        }
                        Err(error) => {
                            log::warn!("Tray update check failed: {}", error);
                            if let Err(notify_error) = show_desktop_notification(
                                &app_handle,
                                "Rust-Srec".to_string(),
                                format!("Failed to check for updates: {error}"),
                            )
                            .await
                            {
                                log::warn!(
                                    "Failed to show update error notification: {}",
                                    notify_error
                                );
                            }
                        }
                    }
                });
            }
        })
        .on_tray_icon_event(move |tray: &tauri::tray::TrayIcon, event| {
            // A double-click must leave the window shown, but it does not
            // arrive as one event: Windows reports click, double-click, click,
            // and macOS reports two clicks and no double-click. So a click
            // toggles the window only when no other click came just before it.
            let double_click = match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => false,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => true,
                _ => return,
            };
            let now = Instant::now();
            let follows_click = last_left_click
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .replace(now)
                .is_some_and(|previous| now.duration_since(previous) < TRAY_DOUBLE_CLICK_WINDOW);
            if double_click || follows_click {
                show_main_window(tray.app_handle());
            } else {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(TrayController {
        status_item,
        idle_icon,
        recording_icon,
        remote_host,
        last_recording: false,
        last_tooltip: initial_tooltip,
        last_status_label: initial_status_label,
        shutting_down: false,
    })
}

/// Apply changed tray status, tooltip, and icon badge asynchronously on the
/// main event loop so callers on background tasks never block on a main thread
/// parked in `RunEvent::Exit`.
pub(crate) fn update_tray(
    app: &tauri::AppHandle,
    summary: ActiveRecordingSummary,
    shutting_down: bool,
) {
    if !shutting_down && exit_owned(app) {
        return;
    }

    let planned = {
        let Some(state) = app.try_state::<DesktopBackendState>() else {
            return;
        };
        let Ok(mut guard) = state.tray_controller.lock() else {
            return;
        };
        let Some(controller) = guard.as_mut() else {
            return;
        };
        controller.plan_update(summary, shutting_down)
    };

    let Some(update) = planned else {
        return;
    };

    let app_handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if !shutting_down && exit_owned(&app_handle) {
            return;
        }

        if let Some((status_item, label)) = update.status_item
            && let Err(error) = status_item.set_text(label)
        {
            log::warn!("Failed to update tray status menu item: {}", error);
        }

        if let Some(tray) = app_handle.tray_by_id(TRAY_ID) {
            if let Some(tooltip) = update.tooltip
                && let Err(error) = tray.set_tooltip(Some(tooltip))
            {
                log::warn!("Failed to update tray tooltip: {}", error);
            }
            if let Some(icon) = update.icon
                && let Err(error) = tray.set_icon(Some(icon))
            {
                log::warn!("Failed to update tray icon: {}", error);
            }
        }
    }) {
        log::warn!("Failed to dispatch tray update on main thread: {}", error);
    }
}

/// Identifier of the application menu's Quit item.
pub(crate) const APP_MENU_QUIT_ID: &str = "app-menu-quit";

/// Tauri's default macOS menu with Quit routed through this app.
///
/// The predefined Quit item sends AppKit `terminate:`, which never raises
/// `ExitRequested`, so Cmd-Q and the application menu would skip the backend
/// drain that the tray's Quit performs. A regular item with the same key
/// equivalent goes through `AppHandle::exit` instead. Everything else mirrors
/// `Menu::default` so the Edit key equivalents keep reaching the webview and
/// the Window and Help submenus keep their AppKit roles.
pub(crate) fn app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{
        AboutMetadata, HELP_SUBMENU_ID, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID,
    };

    let package = app.package_info();
    let bundle = &app.config().bundle;
    let about = AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: bundle.copyright.clone(),
        authors: bundle.publisher.clone().map(|publisher| vec![publisher]),
        ..Default::default()
    };
    let quit = MenuItem::with_id(
        app,
        APP_MENU_QUIT_ID,
        format!("Quit {}", package.name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;

    let application = Submenu::with_items(
        app,
        package.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[&PredefinedMenuItem::close_window(app, None)?],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let help = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &[])?;

    Menu::with_items(app, &[&application, &file, &edit, &view, &window, &help])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_recording_badge_blends_over_transparent_pixels_and_handles_edge_sizes() {
        // Transparent 32x32 buffer exercises alpha blending when `dst_a == 0`.
        let width = 32_u32;
        let height = 32_u32;
        let transparent = vec![0_u8; (width * height * 4) as usize];
        let badged = composite_recording_badge(&transparent, width, height);
        assert_eq!(badged.len(), transparent.len());

        // Top-left quadrant remains untouched.
        for y in 0..16_usize {
            for x in 0..16_usize {
                let idx = (y * width as usize + x) * 4;
                assert_eq!(&badged[idx..idx + 4], &[0, 0, 0, 0]);
            }
        }

        // Bottom-right quadrant contains composited opaque red badge pixels.
        let has_red_badge_pixel = (16..32_usize).any(|y| {
            (16..32_usize).any(|x| {
                let idx = (y * width as usize + x) * 4;
                badged[idx] > 200
                    && badged[idx + 1] < 100
                    && badged[idx + 2] < 100
                    && badged[idx + 3] == 255
            })
        });
        assert!(has_red_badge_pixel, "expected red badge in bottom-right");

        // Tiny icons (where min_dim <= 5) must not panic on radius clamping.
        let tiny = vec![0_u8; 4 * 4 * 4];
        assert_eq!(composite_recording_badge(&tiny, 4, 4).len(), tiny.len());

        // Zero-dimension and truncated buffers return safely without panicking.
        assert!(composite_recording_badge(&[], 0, 0).is_empty());
        let truncated = vec![1_u8; 10];
        assert_eq!(composite_recording_badge(&truncated, 8, 8), truncated);
    }
}
