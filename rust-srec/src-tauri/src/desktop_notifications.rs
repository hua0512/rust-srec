use std::io;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use rust_srec::backend::{NotificationEvent, NotificationPriority};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tauri::Listener;
use tauri::Manager;
#[cfg(not(windows))]
use tauri_plugin_notification::NotificationExt;

use crate::DesktopBackendState;
use crate::json_file::write_json_atomically;

pub const EVENT_SET_DESKTOP_NOTIFICATIONS: &str = "rust-srec://desktop-notifications-set";
pub const EVENT_UPDATED_DESKTOP_NOTIFICATIONS: &str = "rust-srec://desktop-notifications-updated";

#[cfg(windows)]
mod windows;

/// Use the same delivery path for the test button and backend events.
pub async fn show_desktop_notification(
    app: &tauri::AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let app_id = app.config().identifier.clone();
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        // Registry/file IO and WinRT delivery must not block the UI or an async worker.
        tauri::async_runtime::spawn_blocking(move || {
            windows::show(&app_id, "Rust-Srec", &data_dir, &title, &body)
                .map_err(|error| format!("{error:#}"))
        })
        .await
        .map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        app.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub async fn test_desktop_notifications(app: tauri::AppHandle) -> Result<(), String> {
    show_desktop_notification(&app, "Rust-Srec".into(), "Test desktop notification".into()).await
}

/// Event types that should trigger native desktop notifications by default.
/// Focus on "stream online" and error conditions.
pub const DEFAULT_DESKTOP_NOTIFICATION_EVENT_TYPES: &[&str] = &[
    "stream_online",
    "download_error",
    "pipeline_failed",
    "pipeline_queue_critical",
    "fatal_error",
    "out_of_space",
    "credential_refresh_failed",
    "credential_invalid",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationConfig {
    pub enabled: bool,
    pub min_priority: NotificationPriority,
    pub event_types: Vec<String>,
}

impl Default for DesktopNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_priority: NotificationPriority::Normal,
            event_types: DEFAULT_DESKTOP_NOTIFICATION_EVENT_TYPES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

pub fn load_or_create_desktop_notifications_config(
    data_dir: &Path,
) -> io::Result<DesktopNotificationConfig> {
    let config_path = desktop_notifications_config_path(data_dir);
    if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path)?;
        let cfg = serde_json::from_str::<DesktopNotificationConfig>(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        return Ok(cfg);
    }

    let cfg = DesktopNotificationConfig::default();
    persist_desktop_notifications_config(data_dir, &cfg)?;
    Ok(cfg)
}

pub fn persist_desktop_notifications_config(
    data_dir: &Path,
    cfg: &DesktopNotificationConfig,
) -> io::Result<()> {
    write_json_atomically(&desktop_notifications_config_path(data_dir), cfg)
}

pub fn should_deliver_desktop_notification(
    cfg: &DesktopNotificationConfig,
    event: &NotificationEvent,
) -> bool {
    if !cfg.enabled {
        return false;
    }

    if event.priority() < cfg.min_priority {
        return false;
    }

    if cfg.event_types.is_empty() {
        return false;
    }

    cfg.event_types.iter().any(|t| t == event.event_type())
}

pub fn register_desktop_notifications_ipc(app_handle: &tauri::AppHandle) {
    {
        let handle = app_handle.clone();
        let _ = app_handle.listen(EVENT_SET_DESKTOP_NOTIFICATIONS, move |event| {
            let payload = event.payload();
            if payload.trim().is_empty() {
                return;
            }

            let parsed = match serde_json::from_str::<DesktopNotificationConfig>(payload) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("Invalid desktop notifications config payload: {}", e);
                    return;
                }
            };

            let state = handle.state::<DesktopBackendState>();
            state.set_desktop_notifications(parsed.clone());
            state.persist_desktop_notifications_async();

            if let Err(e) = handle.emit(EVENT_UPDATED_DESKTOP_NOTIFICATIONS, parsed) {
                log::warn!("Failed to emit desktop notifications update: {}", e);
            }
        });
    }
}

fn desktop_notifications_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("desktop_notifications.json")
}

impl DesktopBackendState {
    pub fn desktop_notifications(&self) -> DesktopNotificationConfig {
        self.desktop_notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_desktop_notifications(&self, config: DesktopNotificationConfig) {
        *self
            .desktop_notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = config;
    }

    /// Write the current config to disk on a blocking thread.
    ///
    /// The config is read after the write lock is taken, so when two saves
    /// overlap the file ends up with the newer config whichever runs last.
    pub fn persist_desktop_notifications_async(&self) {
        let data_dir = self.data_dir.clone();
        let config = self.desktop_notifications.clone();
        let write_lock = self.notifications_write_lock.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = write_lock.lock();
            let snapshot = config
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Err(e) = persist_desktop_notifications_config(&data_dir, &snapshot) {
                log::warn!("Failed to persist desktop notifications config: {}", e);
            }
        });
    }
}
