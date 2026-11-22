use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::DesktopBackendState;
use crate::desktop_notifications::DesktopNotificationConfig;
use crate::window_state::{center_main_window_once, show_main_window};

#[derive(Clone, Serialize)]
pub(crate) struct BootProgressPayload {
    pub(crate) status: String,
    pub(crate) progress: f32, // 0.0 to 1.0
}

impl Default for BootProgressPayload {
    fn default() -> Self {
        Self {
            status: "Starting...".to_string(),
            progress: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BootFailureStage {
    Logging,
    Database,
    Migrations,
    Backend,
    Services,
    ApiServer,
    Window,
}

impl BootFailureStage {
    fn is_database(self) -> bool {
        matches!(self, Self::Database | Self::Migrations)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BootFailureKind {
    DatabaseBusy,
    PermissionDenied,
    StorageFull,
    DatabaseCorrupt,
    MigrationFailed,
    LoggingFailed,
    ServiceFailed,
    ApiServerFailed,
    WindowFailed,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootFailurePayload {
    stage: BootFailureStage,
    kind: BootFailureKind,
    title: String,
    message: String,
    details: String,
}

impl BootFailurePayload {
    pub(crate) fn new(stage: BootFailureStage, details: impl Into<String>) -> Self {
        let details = details.into();
        let kind = classify_boot_failure(stage, &details);
        let (title, message) = boot_failure_copy(kind);

        Self {
            stage,
            kind,
            title: title.to_string(),
            message: message.to_string(),
            details,
        }
    }
}

pub(crate) fn classify_boot_failure(stage: BootFailureStage, details: &str) -> BootFailureKind {
    let normalized = details.to_ascii_lowercase();

    if stage.is_database()
        && [
            "database is locked",
            "database is busy",
            "another rust-srec runtime already owns",
            "(code: 5)",
            "(code: 6)",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return BootFailureKind::DatabaseBusy;
    }

    if [
        "permission denied",
        "access is denied",
        "read-only",
        "readonly",
        "attempt to write a readonly database",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return BootFailureKind::PermissionDenied;
    }

    if ["database or disk is full", "no space left on device"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return BootFailureKind::StorageFull;
    }

    if stage.is_database()
        && [
            "database disk image is malformed",
            "file is not a database",
            "database corruption",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return BootFailureKind::DatabaseCorrupt;
    }

    match stage {
        BootFailureStage::Logging => BootFailureKind::LoggingFailed,
        BootFailureStage::Migrations => BootFailureKind::MigrationFailed,
        BootFailureStage::Backend | BootFailureStage::Services => BootFailureKind::ServiceFailed,
        BootFailureStage::ApiServer => BootFailureKind::ApiServerFailed,
        BootFailureStage::Window => BootFailureKind::WindowFailed,
        BootFailureStage::Database => BootFailureKind::Unknown,
    }
}

fn boot_failure_copy(kind: BootFailureKind) -> (&'static str, &'static str) {
    match kind {
        BootFailureKind::DatabaseBusy => (
            "The database is temporarily unavailable",
            "Another process is using the application database. Close other Rust-Srec windows or database tools, then restart.",
        ),
        BootFailureKind::PermissionDenied => (
            "Rust-Srec cannot access its files",
            "Check the application data folder permissions and security software, then restart.",
        ),
        BootFailureKind::StorageFull => (
            "There is not enough free disk space",
            "Free some space on the drive containing the application data, then restart.",
        ),
        BootFailureKind::DatabaseCorrupt => (
            "The database may be damaged",
            "Do not delete the database. Open the data folder and make a backup before attempting recovery.",
        ),
        BootFailureKind::MigrationFailed => (
            "The database could not be upgraded",
            "Review the logs before restarting. The existing database has not been reset.",
        ),
        BootFailureKind::LoggingFailed => (
            "Logging could not start",
            "Check that the log folder is writable and that the drive has free space.",
        ),
        BootFailureKind::ServiceFailed => (
            "A desktop service could not start",
            "Restart the application. If the problem continues, copy the details and review the logs.",
        ),
        BootFailureKind::ApiServerFailed => (
            "The local API could not start",
            "Restart the application. If the problem continues, review the logs for the underlying service error.",
        ),
        BootFailureKind::WindowFailed => (
            "The desktop window could not be created",
            "Restart the application. Updating the system webview may be required if the problem continues.",
        ),
        BootFailureKind::Unknown => (
            "Rust-Srec could not finish starting",
            "Restart the application. If the problem continues, copy the details and review the logs.",
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopRecoveryLocation {
    Data,
    Logs,
}

pub(crate) fn emit_boot_progress(app: &tauri::AppHandle, status: &str, progress: f32) {
    let payload = app
        .try_state::<DesktopBackendState>()
        .map(|state| state.set_boot_progress(status, progress))
        .unwrap_or(BootProgressPayload {
            status: status.to_string(),
            progress: progress.clamp(0.0, 1.0),
        });

    // Always use global emit for boot progress to ensure the splash window catches it
    // regardless of whether it's listening locally or globally.
    if let Err(e) = app.emit("boot-progress", payload) {
        log::warn!("Failed to emit boot progress: {}", e);
    }
}

pub(crate) fn build_init_script(
    backend_url: &str,
    boot_error: Option<&BootFailurePayload>,
    desktop_notifications: &DesktopNotificationConfig,
) -> String {
    let backend_url_json =
        serde_json::to_string(backend_url).unwrap_or_else(|_| "\"\"".to_string());
    let boot_error_json = match boot_error {
        Some(error) => serde_json::to_string(error).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };

    let desktop_notifications_json =
        serde_json::to_string(desktop_notifications).unwrap_or_else(|_| "null".to_string());

    format!(
        "globalThis.__RUST_SREC_BACKEND_URL__ = {backend_url_json};\
 globalThis.__RUST_SREC_BOOT_ERROR__ = {boot_error_json};\
 globalThis.__RUST_SREC_DESKTOP_NOTIFICATIONS__ = {desktop_notifications_json};"
    )
}

pub(crate) async fn show_boot_error_window(
    app_handle: &tauri::AppHandle,
    failure: BootFailurePayload,
) {
    log::error!(
        "Desktop boot failed at {:?} ({:?}): {}",
        failure.stage,
        failure.kind,
        failure.details
    );

    let state = app_handle.state::<DesktopBackendState>();
    if state.init_cancel.is_cancelled() {
        // The exit path already owns the process; a recovery window would only
        // flash before it closes.
        return;
    }
    state.mark_boot_failed();
    let desktop_notifications = state.desktop_notifications();

    let webview_url = if tauri::is_dev() {
        match tauri::Url::parse("http://127.0.0.1:15275/index.desktop.html") {
            Ok(url) => tauri::WebviewUrl::External(url),
            Err(_) => tauri::WebviewUrl::App("index.desktop.html".into()),
        }
    } else {
        tauri::WebviewUrl::App("index.desktop.html".into())
    };

    let init_script = build_init_script("", Some(&failure), &desktop_notifications);

    if app_handle.get_webview_window("main").is_none() {
        match tauri::WebviewWindowBuilder::new(app_handle, "main", webview_url)
            .title("Rust-Srec - Startup error")
            .inner_size(920.0, 720.0)
            .min_inner_size(640.0, 520.0)
            .initialization_script(init_script)
            .build()
        {
            Ok(window) => {
                center_main_window_once(app_handle);
                if let Err(error) = window.show() {
                    log::error!("Failed to show desktop boot error window: {}", error);
                }
                if let Err(error) = window.set_focus() {
                    log::warn!("Failed to focus desktop boot error window: {}", error);
                }
            }
            Err(error) => {
                log::error!("Failed to create desktop boot error window: {}", error);
            }
        }
    } else {
        show_main_window(app_handle);
    }

    if let Some(splash) = app_handle.get_webview_window("splash")
        && let Err(error) = splash.close()
    {
        log::warn!("Failed to close splash after boot failure: {}", error);
    }
}

#[tauri::command]
pub fn open_desktop_recovery_location(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
    location: DesktopRecoveryLocation,
) -> Result<(), String> {
    let path = match location {
        DesktopRecoveryLocation::Data => &state.data_dir,
        DesktopRecoveryLocation::Logs => &state.log_dir,
    };
    let path_display = path.to_string_lossy().into_owned();

    app.opener()
        .open_path(path_display.clone(), None::<&str>)
        .map_err(|error| format!("Failed to open '{path_display}': {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transient_and_runtime_lease_database_locks() {
        for detail in [
            "Failed to open database: (code: 5) database is locked",
            "Failed to acquire database lock: Configuration error: another rust-srec runtime already owns D:\\srec.db.runtime-generation.dirty.lock",
        ] {
            let failure = BootFailurePayload::new(BootFailureStage::Database, detail);
            assert_eq!(failure.kind, BootFailureKind::DatabaseBusy);
            assert!(failure.message.contains("Close other Rust-Srec windows"));
        }
    }

    #[test]
    fn classifies_database_corruption_without_destructive_advice() {
        let failure = BootFailurePayload::new(
            BootFailureStage::Migrations,
            "database disk image is malformed",
        );

        assert_eq!(failure.kind, BootFailureKind::DatabaseCorrupt);
        assert!(failure.message.contains("Do not delete the database"));
        assert!(failure.message.contains("make a backup"));
    }

    #[test]
    fn classifies_permission_and_storage_failures_across_stages() {
        assert_eq!(
            classify_boot_failure(BootFailureStage::Logging, "Access is denied"),
            BootFailureKind::PermissionDenied
        );
        assert_eq!(
            classify_boot_failure(
                BootFailureStage::Database,
                "Failed to acquire database lock: Permission denied (os error 13)",
            ),
            BootFailureKind::PermissionDenied
        );
        assert_eq!(
            classify_boot_failure(BootFailureStage::Services, "No space left on device"),
            BootFailureKind::StorageFull
        );
    }

    #[test]
    fn uses_stage_specific_fallback_for_migration_errors() {
        assert_eq!(
            classify_boot_failure(BootFailureStage::Migrations, "unexpected schema version"),
            BootFailureKind::MigrationFailed
        );
    }
}
