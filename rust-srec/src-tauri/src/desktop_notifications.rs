use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tauri::Listener;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::DesktopBackendState;

pub const EVENT_SET_DESKTOP_NOTIFICATIONS: &str = "rust-srec://desktop-notifications-set";
pub const EVENT_UPDATED_DESKTOP_NOTIFICATIONS: &str = "rust-srec://desktop-notifications-updated";
pub const EVENT_TEST_DESKTOP_NOTIFICATIONS: &str = "rust-srec://desktop-notifications-test";

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

/// Desktop notification minimum priority.
///
/// Serializes as integer (Gotify-compatible 0-10 scale).
/// Deserializes from integer or legacy string ("low"/"normal"/"high"/"critical").
#[derive(Debug, Default, Clone, Copy)]
pub enum DesktopNotificationMinPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl DesktopNotificationMinPriority {
    fn as_int(self) -> u8 {
        match self {
            Self::Low => 2,
            Self::Normal => 5,
            Self::High => 8,
            Self::Critical => 10,
        }
    }

    fn from_int(value: u8) -> Option<Self> {
        match value {
            0..=3 => Some(Self::Low),
            4..=6 => Some(Self::Normal),
            7..=9 => Some(Self::High),
            10..=u8::MAX => Some(Self::Critical),
        }
    }
}

impl Serialize for DesktopNotificationMinPriority {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.as_int())
    }
}

impl<'de> Deserialize<'de> for DesktopNotificationMinPriority {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = DesktopNotificationMinPriority;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("integer (0-10) or string (low/normal/high/critical)")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                let val = u8::try_from(v)
                    .map_err(|_| de::Error::invalid_value(de::Unexpected::Unsigned(v), &self))?;
                DesktopNotificationMinPriority::from_int(val)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Unsigned(v), &self))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                let val = u8::try_from(v)
                    .map_err(|_| de::Error::invalid_value(de::Unexpected::Signed(v), &self))?;
                DesktopNotificationMinPriority::from_int(val)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Signed(v), &self))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "low" => Ok(DesktopNotificationMinPriority::Low),
                    "normal" => Ok(DesktopNotificationMinPriority::Normal),
                    "high" => Ok(DesktopNotificationMinPriority::High),
                    "critical" => Ok(DesktopNotificationMinPriority::Critical),
                    _ => Err(de::Error::unknown_variant(
                        v,
                        &["low", "normal", "high", "critical"],
                    )),
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl From<DesktopNotificationMinPriority> for rust_srec::notification::NotificationPriority {
    fn from(value: DesktopNotificationMinPriority) -> Self {
        match value {
            DesktopNotificationMinPriority::Low => Self::Low,
            DesktopNotificationMinPriority::Normal => Self::Normal,
            DesktopNotificationMinPriority::High => Self::High,
            DesktopNotificationMinPriority::Critical => Self::Critical,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationConfig {
    pub enabled: bool,
    pub min_priority: DesktopNotificationMinPriority,
    pub event_types: Vec<String>,
}

impl Default for DesktopNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_priority: DesktopNotificationMinPriority::Normal,
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
    let config_path = desktop_notifications_config_path(data_dir);
    let tmp_path = config_path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp_path, json)?;
    if config_path.exists() {
        let _ = std::fs::remove_file(&config_path);
    }
    std::fs::rename(&tmp_path, &config_path)?;
    Ok(())
}

pub fn should_deliver_desktop_notification(
    cfg: &DesktopNotificationConfig,
    event: &rust_srec::notification::NotificationEvent,
) -> bool {
    if !cfg.enabled {
        return false;
    }

    let min_priority: rust_srec::notification::NotificationPriority = cfg.min_priority.into();
    if event.priority() < min_priority {
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
            if let Err(e) = state.persist_desktop_notifications() {
                log::warn!("Failed to persist desktop notifications config: {}", e);
            }

            if let Err(e) = handle.emit(EVENT_UPDATED_DESKTOP_NOTIFICATIONS, parsed) {
                log::warn!("Failed to emit desktop notifications update: {}", e);
            }
        });
    }

    {
        let handle = app_handle.clone();
        let _ = app_handle.listen(EVENT_TEST_DESKTOP_NOTIFICATIONS, move |_event| {
            if let Err(e) = handle
                .notification()
                .builder()
                .title("Rust-Srec")
                .body("Test desktop notification")
                .show()
            {
                log::warn!("Failed to show desktop test notification: {}", e);
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
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    pub fn set_desktop_notifications(&self, config: DesktopNotificationConfig) {
        if let Ok(mut lock) = self.desktop_notifications.lock() {
            *lock = config;
        }
    }

    pub fn persist_desktop_notifications(&self) -> io::Result<()> {
        let cfg = self.desktop_notifications();
        persist_desktop_notifications_config(&self.data_dir, &cfg)
    }
}
