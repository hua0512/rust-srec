use std::io;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Emitter;
#[cfg(desktop)]
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

use crate::DesktopBackendState;
use crate::desktop_media::{
    open_validated_directory, output_folder_root, validate_existing_directory,
};
use crate::json_file::write_json_atomically;

pub const EVENT_DESKTOP_SETTINGS_UPDATED: &str = "rust-srec://desktop-settings-updated";
const REMOTE_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_HEALTH_ERROR_BODY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    #[default]
    Local,
    Remote,
}

/// Settings of the desktop shell, stored as `desktop_settings.json`.
///
/// `require_login` defaults to off: the shell signs the webview in without a
/// password. That is safe on a fresh install whose `admin` still has the
/// seeded default password because the built-in backend always runs in local
/// desktop mode (`ServiceContainer::enable_local_desktop_mode`, set before the
/// API starts): a password login for an account that must still change its
/// password never succeeds there, so the seeded password never signs in.
///
/// The stored `require_login` is what the user asked for. In local mode
/// password sign-in is required only while it is on *and* the auto-login
/// account has a password of its own ([`password_sign_in_required`]); until a
/// password is set (`set_desktop_password`) the app keeps signing in
/// automatically, whatever the file or a restored backup says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub prevent_sleep_while_recording: bool,
    pub confirm_exit_while_recording: bool,
    pub require_login: bool,
    pub check_updates_on_startup: bool,
    pub skipped_update_version: Option<String>,
    pub backend_mode: BackendMode,
    pub remote_url: Option<String>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            start_minimized: false,
            close_to_tray: true,
            minimize_to_tray: true,
            prevent_sleep_while_recording: true,
            confirm_exit_while_recording: true,
            require_login: false,
            check_updates_on_startup: true,
            skipped_update_version: None,
            backend_mode: BackendMode::Local,
            remote_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettingsSnapshot {
    /// The stored settings, except that `require_login` is the effective
    /// value ([`password_sign_in_required`]): off while the local auto-login
    /// account has no password of its own, so the toggle shows what happens
    /// and the next save writes that value back.
    #[serde(flatten)]
    pub settings: DesktopSettings,
    pub autostart_enabled: bool,
    pub data_dir: String,
    pub log_dir: String,
    pub output_dir: Option<String>,
    pub active_backend_mode: BackendMode,
    pub app_version: String,
    /// The auto-login account still has its initial password, so a password
    /// has to be set before `require_login` can be turned on.
    pub password_setup_required: bool,
    /// The account auto-login signs in as. `None` in remote mode, before the
    /// local backend is running, or when it has no active admin.
    pub local_username: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnectionCheck {
    pub normalized_url: String,
    pub healthy: bool,
    pub status_code: u16,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAutoLoginSession {
    pub username: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub refresh_expires_in: u64,
    pub roles: Vec<String>,
    pub must_change_password: bool,
}

/// Validate and normalize a remote Rust-Srec backend URL.
///
/// Requires `http://` or `https://` with a non-empty host, strips any trailing
/// slash, and removes a trailing `/api` segment so frontend API requests do not
/// end up targeting `/api/api`.
pub fn normalize_remote_backend_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Remote server URL cannot be empty".to_string());
    }

    let mut parsed = tauri::Url::parse(trimmed)
        .map_err(|error| format!("Invalid remote server URL: {error}"))?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Remote server URL must use http:// or https://".to_string());
    }

    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("Remote server URL must include a host".to_string());
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Remote server URL must not include embedded credentials".to_string());
    }

    parsed.set_query(None);
    parsed.set_fragment(None);

    let trimmed_path = parsed.path().trim_end_matches('/');
    let normalized_path = trimmed_path
        .strip_suffix("/api")
        .unwrap_or(trimmed_path)
        .to_string();
    parsed.set_path(&normalized_path);

    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

fn desktop_settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("desktop_settings.json")
}

fn load_or_create_desktop_settings(data_dir: &Path) -> io::Result<DesktopSettings> {
    let path = desktop_settings_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<DesktopSettings>(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let settings = DesktopSettings::default();
            persist_desktop_settings(data_dir, &settings)?;
            Ok(settings)
        }
        Err(e) => Err(e),
    }
}

/// The stored settings, or the defaults when the file cannot be read or
/// parsed. That fallback keeps `require_login` on: the file may be the only
/// record that the user asked for password sign-in, and with
/// [`password_sign_in_required`] it costs a user who never set a password
/// nothing.
pub fn load_desktop_settings_or_fallback(data_dir: &Path) -> DesktopSettings {
    match load_or_create_desktop_settings(data_dir) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!(
                "Failed to load desktop settings, using defaults with password sign-in required: {}",
                error
            );
            DesktopSettings {
                require_login: true,
                ..DesktopSettings::default()
            }
        }
    }
}

/// Whether the local backend requires password sign-in: the user asked for
/// it (`require_login`) and the auto-login account has a password of its own.
/// An account that cannot be looked up (`None`) leaves the stored value as it
/// stands, so a lookup failure never drops the requirement.
pub(crate) fn password_sign_in_required(
    require_login: bool,
    account: Option<&rust_srec::backend::LocalDesktopAccount>,
) -> bool {
    require_login && !account.is_some_and(|account| account.password_setup_required)
}

/// The auto-login account of the running local backend, or `None` without a
/// backend, without an active admin, or when the lookup fails (logged).
async fn local_desktop_account(
    container: Option<&rust_srec::backend::ServiceContainer>,
) -> Option<rust_srec::backend::LocalDesktopAccount> {
    match container?.local_desktop_account().await {
        Ok(account) => account,
        Err(error) => {
            log::warn!("Failed to look up the local desktop account: {}", error);
            None
        }
    }
}

fn persist_desktop_settings(data_dir: &Path, settings: &DesktopSettings) -> io::Result<()> {
    write_json_atomically(&desktop_settings_path(data_dir), settings)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedLocalDesktopSession {
    session_id: String,
    user_id: String,
}

fn local_desktop_sessions_path(data_dir: &Path) -> PathBuf {
    data_dir.join("local_desktop_sessions.json")
}

/// The `(session_id, user_id)` pairs stored by [`persist_local_desktop_sessions`].
///
/// A missing file is an empty list. A file that cannot be read is an error,
/// because the caller may be about to revoke these sessions and must not take
/// "unreadable" for "none". A file that does not parse holds nothing that can
/// be recovered, so it is logged and read as empty.
fn load_local_desktop_sessions(data_dir: &Path) -> io::Result<Vec<(String, String)>> {
    let raw = match std::fs::read_to_string(local_desktop_sessions_path(data_dir)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    match serde_json::from_str::<Vec<PersistedLocalDesktopSession>>(&raw) {
        Ok(entries) => Ok(entries
            .into_iter()
            .filter(|entry| !entry.session_id.trim().is_empty() && !entry.user_id.trim().is_empty())
            .map(|entry| (entry.session_id, entry.user_id))
            .collect()),
        Err(error) => {
            log::warn!(
                "Stored local desktop sessions are not valid JSON: {}",
                error
            );
            Ok(Vec::new())
        }
    }
}

fn persist_local_desktop_sessions(
    data_dir: &Path,
    sessions: &[(String, String)],
) -> io::Result<()> {
    let path = local_desktop_sessions_path(data_dir);
    if sessions.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }

    let entries: Vec<PersistedLocalDesktopSession> = sessions
        .iter()
        .map(|(session_id, user_id)| PersistedLocalDesktopSession {
            session_id: session_id.clone(),
            user_id: user_id.clone(),
        })
        .collect();
    write_json_atomically(&path, &entries)
}

pub fn should_start_minimized(settings: &DesktopSettings, args: &[String]) -> bool {
    settings.start_minimized || args.iter().any(|arg| arg == "--minimized")
}

async fn resolve_configured_output_folder(state: &DesktopBackendState) -> Option<String> {
    let container = state.backend()?;
    match container.config_service().get_global_config().await {
        Ok(config) => {
            let trimmed = config.output_folder.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(error) => {
            log::warn!(
                "Failed to load global config for desktop output folder: {}",
                error
            );
            None
        }
    }
}

pub(crate) async fn open_recordings_folder_for_app(
    app: &tauri::AppHandle,
    state: &DesktopBackendState,
) -> Result<(), String> {
    if state.active_backend_mode != BackendMode::Local {
        return Err("Not available when connected to a remote server".to_string());
    }

    let candidate = resolve_configured_output_folder(state)
        .await
        .map(|folder| output_folder_root(&folder).to_string())
        .filter(|root| !root.is_empty())
        .unwrap_or_else(|| state.data_dir.join("output").to_string_lossy().into_owned());

    let dir = tauri::async_runtime::spawn_blocking(move || validate_existing_directory(&candidate))
        .await
        .map_err(|e| format!("Failed to validate recordings folder: {e}"))??;

    open_validated_directory(app, &dir)
}

#[tauri::command]
pub async fn get_desktop_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
) -> Result<DesktopSettingsSnapshot, String> {
    let settings = state.desktop_settings();

    #[cfg(desktop)]
    let autostart_enabled = {
        let app_handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app_handle.autolaunch().is_enabled().unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    };
    #[cfg(not(desktop))]
    let autostart_enabled = {
        let _ = &app;
        false
    };

    let output_dir = resolve_configured_output_folder(&state).await;
    let local_account = local_desktop_account(state.backend().as_deref()).await;
    let password_setup_required = local_account
        .as_ref()
        .is_some_and(|account| account.password_setup_required);
    let settings = DesktopSettings {
        require_login: password_sign_in_required(settings.require_login, local_account.as_ref()),
        ..settings
    };

    Ok(DesktopSettingsSnapshot {
        settings,
        autostart_enabled,
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        log_dir: state.log_dir.to_string_lossy().into_owned(),
        output_dir,
        active_backend_mode: state.active_backend_mode,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        password_setup_required,
        local_username: local_account.map(|account| account.username),
    })
}

#[tauri::command]
pub async fn update_desktop_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
    mut settings: DesktopSettings,
) -> Result<DesktopSettings, String> {
    let raw_remote_url = settings
        .remote_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    settings.remote_url = match (settings.backend_mode, raw_remote_url) {
        (BackendMode::Remote, Some(url)) => Some(normalize_remote_backend_url(url)?),
        (BackendMode::Remote, None) => {
            return Err("Remote server URL is required when remote mode is enabled".to_string());
        }
        // In local mode the address is only kept for switching back later, so
        // one that does not parse is dropped instead of failing the save.
        (BackendMode::Local, url) => url.and_then(|url| normalize_remote_backend_url(url).ok()),
    };

    let _write_guard = state.settings_write_lock.lock().await;

    // Only `skip_update_version` changes the skipped version. The frontend
    // sends the whole struct back, so a write queued before a skip would
    // otherwise undo it.
    settings.skipped_update_version = state.desktop_settings().skipped_update_version;

    // Without a password of its own the account could only be signed in to
    // with the seeded default, so turning sign-in on asks for a password
    // first (`set_desktop_password`). Storing `true` regardless would be safe
    // ([`password_sign_in_required`] ignores it until a password exists);
    // refusing tells the user why nothing changed. In remote mode there is no
    // local account, and the value is stored as given.
    let account = local_desktop_account(state.backend().as_deref()).await;
    if settings.require_login
        && account
            .as_ref()
            .is_some_and(|account| account.password_setup_required)
    {
        return Err(
            "Set a password before turning on password sign-in: this account has no password of its own yet"
                .to_string(),
        );
    }
    if password_sign_in_required(settings.require_login, account.as_ref()) {
        revoke_local_desktop_sessions(&state).await?;
    }
    state.commit_desktop_settings(&app, &settings).await
}

/// Revoke every session issued by auto-login and forget the stored list.
///
/// The caller holds `settings_write_lock`, which `desktop_auto_login` also
/// takes, and calls this before it stores `require_login`: no session can be
/// issued in between, and a failure leaves the setting as it was. Without a
/// running local backend (remote mode) nothing can be revoked now, so the
/// stored list is left for [`reconcile_local_desktop_sessions`] at the next
/// local start.
async fn revoke_local_desktop_sessions(state: &DesktopBackendState) -> Result<(), String> {
    let Some(container) = state.backend() else {
        return Ok(());
    };

    let data_dir = state.data_dir.clone();
    let persisted =
        tauri::async_runtime::spawn_blocking(move || load_local_desktop_sessions(&data_dir))
            .await
            .map_err(|e| format!("Failed to read stored desktop sessions: {e}"))?
            .map_err(|e| format!("Failed to read stored desktop sessions: {e}"))?;

    container
        .revoke_local_desktop_sessions(&persisted)
        .await
        .map_err(|e| format!("Failed to sign out existing desktop sessions: {e}"))?;

    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || persist_local_desktop_sessions(&data_dir, &[]))
        .await
        .map_err(|e| format!("Failed to clear stored desktop sessions: {e}"))?
        .map_err(|e| format!("Failed to clear stored desktop sessions: {e}"))
}

/// Bring the stored auto-login sessions in line with
/// [`password_sign_in_required`] once the local backend is up: revoke them
/// while password sign-in is required (it may have been switched on while this
/// backend was not running), otherwise track them again so they keep working
/// across the restart.
pub(crate) async fn reconcile_local_desktop_sessions(
    state: &DesktopBackendState,
    container: &rust_srec::backend::ServiceContainer,
) {
    let _write_guard = state.settings_write_lock.lock().await;

    let account = local_desktop_account(Some(container)).await;
    if password_sign_in_required(state.desktop_settings().require_login, account.as_ref()) {
        if let Err(error) = revoke_local_desktop_sessions(state).await {
            log::warn!("{error}; retrying at the next start");
        }
        return;
    }

    let data_dir = state.data_dir.clone();
    match tauri::async_runtime::spawn_blocking(move || load_local_desktop_sessions(&data_dir)).await
    {
        Ok(Ok(persisted)) => container.restore_local_desktop_sessions(persisted),
        Ok(Err(error)) => log::warn!("Failed to read stored desktop sessions: {error}"),
        Err(error) => log::warn!("Failed to read stored desktop sessions: {error}"),
    }
}

/// The server address a redirected health check points at, when the redirect
/// still leads to a health endpoint (an `http` to `https` upgrade, a moved
/// subpath). `None` for any other target, such as a sign-in page.
fn redirected_backend_url(health_url: &str, location: &str) -> Option<String> {
    let mut target = tauri::Url::parse(health_url).ok()?.join(location).ok()?;
    let base_path = target
        .path()
        .trim_end_matches('/')
        .strip_suffix("/api/health")?
        .to_string();
    target.set_path(&base_path);
    normalize_remote_backend_url(target.as_str()).ok()
}

#[tauri::command]
pub async fn test_remote_backend_connection(url: String) -> Result<RemoteConnectionCheck, String> {
    let normalized_url = normalize_remote_backend_url(&url)?;

    let client = reqwest::Client::builder()
        .timeout(REMOTE_HEALTH_CHECK_TIMEOUT)
        // The frontend talks to exactly the address that gets saved, so a
        // redirect is reported instead of followed.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("rust-srec-desktop/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Failed to initialize HTTP client: {error}"))?;

    let health_url = format!("{normalized_url}/api/health");
    let mut response = client
        .get(&health_url)
        .send()
        .await
        .map_err(|error| format!("Failed to reach {health_url}: {error}"))?;

    let status = response.status();
    let healthy = status.is_success();
    let detail = if healthy {
        None
    } else if status.is_redirection() {
        let suggestion = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| redirected_backend_url(&health_url, location));
        Some(match suggestion {
            Some(address) => format!(
                "Server returned HTTP {} and redirects to {address}; enter that address instead",
                status.as_u16()
            ),
            None => format!(
                "Server returned HTTP {} and redirects elsewhere; check the address",
                status.as_u16()
            ),
        })
    } else {
        let mut buf = Vec::with_capacity(MAX_HEALTH_ERROR_BODY_BYTES.min(1024));
        while buf.len() < MAX_HEALTH_ERROR_BODY_BYTES {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let remaining = MAX_HEALTH_ERROR_BODY_BYTES - buf.len();
                    buf.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
                Ok(None) | Err(_) => break,
            }
        }
        let body = String::from_utf8_lossy(&buf);
        let snippet = body.trim();
        if snippet.is_empty() {
            Some(format!("Server returned HTTP {}", status.as_u16()))
        } else {
            let truncated: String = snippet.chars().take(200).collect();
            Some(format!(
                "Server returned HTTP {}: {truncated}",
                status.as_u16()
            ))
        }
    };

    Ok(RemoteConnectionCheck {
        normalized_url,
        healthy,
        status_code: status.as_u16(),
        detail,
    })
}

#[tauri::command]
pub async fn desktop_auto_login(
    state: tauri::State<'_, DesktopBackendState>,
) -> Result<Option<DesktopAutoLoginSession>, String> {
    let _write_guard = state.settings_write_lock.lock().await;

    if state.active_backend_mode != BackendMode::Local {
        return Ok(None);
    }
    let container = state.backend();
    let account = local_desktop_account(container.as_deref()).await;
    if password_sign_in_required(state.desktop_settings().require_login, account.as_ref()) {
        return Ok(None);
    }
    let Some(container) = container else {
        return Err("Local backend is not ready yet".to_string());
    };

    let (username, auth) = container
        .issue_local_desktop_session()
        .await
        .map_err(|error| format!("Failed to issue local desktop session: {error}"))?;

    // A session missing from the stored list would be forgotten at the next
    // restart and then survive `require_login` being switched on. So when the
    // list cannot be written, the session is revoked again instead of used.
    let sessions = container.list_local_desktop_sessions();
    let data_dir = state.data_dir.clone();
    let stored = tauri::async_runtime::spawn_blocking(move || {
        persist_local_desktop_sessions(&data_dir, &sessions)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|written| written.map_err(|e| e.to_string()));
    if let Err(error) = stored {
        if let Err(revoke_error) = container.revoke_local_desktop_sessions(&[]).await {
            log::warn!(
                "Failed to revoke a local desktop session that could not be stored: {}",
                revoke_error
            );
        }
        return Err(format!(
            "Failed to store the local desktop session: {error}"
        ));
    }

    Ok(Some(DesktopAutoLoginSession {
        username,
        access_token: auth.access_token,
        refresh_token: auth.refresh_token,
        expires_in: auth.expires_in,
        refresh_expires_in: auth.refresh_expires_in,
        roles: auth.roles,
        must_change_password: auth.must_change_password,
    }))
}

/// Give the auto-login account its first password, replacing the initial one.
/// Refused in remote mode and once the account has a password of its own;
/// changing that needs the current password.
#[tauri::command]
pub async fn set_desktop_password(
    state: tauri::State<'_, DesktopBackendState>,
    password: String,
) -> Result<(), String> {
    if state.active_backend_mode != BackendMode::Local {
        return Err("Not available when connected to a remote server".to_string());
    }
    let _write_guard = state.settings_write_lock.lock().await;
    let Some(container) = state.backend() else {
        return Err("Local backend is not ready yet".to_string());
    };
    container
        .set_local_desktop_password(&password)
        .await
        .map_err(|error| format!("Failed to set the password: {error}"))
}

#[tauri::command]
pub async fn switch_to_local_backend(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
) -> Result<(), String> {
    {
        let _write_guard = state.settings_write_lock.lock().await;
        let mut settings = state.desktop_settings();
        settings.backend_mode = BackendMode::Local;
        state.commit_desktop_settings(&app, &settings).await?;
    }

    crate::lifecycle::request_desktop_restart(app);
    Ok(())
}

#[tauri::command]
pub async fn set_autostart_enabled(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    #[cfg(desktop)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let autolaunch = app.autolaunch();
            if enabled {
                autolaunch
                    .enable()
                    .map_err(|e| format!("Failed to enable autostart: {e}"))?;
            } else {
                autolaunch
                    .disable()
                    .map_err(|e| format!("Failed to disable autostart: {e}"))?;
            }
            autolaunch
                .is_enabled()
                .map_err(|e| format!("Failed to query autostart state: {e}"))
        })
        .await
        .map_err(|e| format!("Failed to update autostart setting: {e}"))?
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, enabled);
        Err("Autostart is only supported on desktop platforms".to_string())
    }
}

#[tauri::command]
pub async fn open_recordings_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
) -> Result<(), String> {
    open_recordings_folder_for_app(&app, &state).await
}

impl DesktopBackendState {
    pub(crate) fn desktop_settings(&self) -> DesktopSettings {
        self.desktop_settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Store `settings` on disk and in memory, then tell the runtime watcher
    /// and the frontend. The caller holds `settings_write_lock`.
    ///
    /// Returns the settings as the frontend sees them, which is also what the
    /// settings-updated event carries: `require_login` is the effective value
    /// ([`password_sign_in_required`]), so a stored `true` that does not apply
    /// yet does not show the toggle as on.
    pub(crate) async fn commit_desktop_settings(
        &self,
        app: &tauri::AppHandle,
        settings: &DesktopSettings,
    ) -> Result<DesktopSettings, String> {
        let data_dir = self.data_dir.clone();
        let to_persist = settings.clone();
        tauri::async_runtime::spawn_blocking(move || {
            persist_desktop_settings(&data_dir, &to_persist)
        })
        .await
        .map_err(|e| format!("Failed to persist desktop settings: {e}"))?
        .map_err(|e| format!("Failed to persist desktop settings: {e}"))?;

        *self
            .desktop_settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = settings.clone();
        self.settings_notify.notify_one();

        let account = local_desktop_account(self.backend().as_deref()).await;
        let reported = DesktopSettings {
            require_login: password_sign_in_required(settings.require_login, account.as_ref()),
            ..settings.clone()
        };
        if let Err(error) = app.emit(EVENT_DESKTOP_SETTINGS_UPDATED, &reported) {
            log::warn!("Failed to emit desktop settings update: {}", error);
        }
        Ok(reported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates_remote_backend_urls() {
        assert_eq!(
            normalize_remote_backend_url("  https://srec.example.com/  ").unwrap(),
            "https://srec.example.com"
        );
        assert_eq!(
            normalize_remote_backend_url("https://srec.example.com/api/").unwrap(),
            "https://srec.example.com"
        );
        assert_eq!(
            normalize_remote_backend_url("http://192.168.1.20:12555/sub/api").unwrap(),
            "http://192.168.1.20:12555/sub"
        );
        assert_eq!(
            normalize_remote_backend_url("https://srec.example.com/apiary/").unwrap(),
            "https://srec.example.com/apiary"
        );

        assert!(normalize_remote_backend_url("   ").is_err());
        assert!(normalize_remote_backend_url("javascript:alert(1)").is_err());
        assert!(normalize_remote_backend_url("file:///etc/passwd").is_err());
        assert!(normalize_remote_backend_url("http://").is_err());
        assert!(normalize_remote_backend_url("https://user:secret@srec.example.com").is_err());
        assert!(normalize_remote_backend_url("https://user@srec.example.com").is_err());
    }

    #[test]
    fn redirected_health_check_suggests_the_server_address_not_the_health_url() {
        let health_url = "http://srec.example.com/sub/api/health";
        assert_eq!(
            redirected_backend_url(health_url, "https://srec.example.com/sub/api/health"),
            Some("https://srec.example.com/sub".to_string())
        );
        assert_eq!(
            redirected_backend_url(health_url, "/moved/api/health/"),
            Some("http://srec.example.com/moved".to_string())
        );
        assert_eq!(redirected_backend_url(health_url, "/login"), None);
    }

    #[test]
    fn unreadable_settings_file_keeps_password_sign_in_required() {
        let data_dir = std::env::temp_dir().join(format!(
            "rust-srec-desktop-settings-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&data_dir).expect("test data dir");
        std::fs::write(desktop_settings_path(&data_dir), "{ not json").expect("corrupt file");

        let settings = load_desktop_settings_or_fallback(&data_dir);
        std::fs::remove_dir_all(&data_dir).expect("remove test data dir");
        assert!(settings.require_login);
    }
}
