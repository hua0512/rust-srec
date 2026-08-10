//! Shared helpers for invoking the BaiduPCS-Go CLI.
//!
//! Used by both `pipeline::processors::baidupcs` (uploads) and
//! `api::routes::baidupcs` (login/status/logout). BaiduPCS-Go persists its
//! login session and upload-state databases inside one config directory
//! (overridable via [`CONFIG_DIR_ENV`]), which is not safe under concurrent
//! writers: [`cli_lock`] serializes session mutation (`login`/`logout` take
//! the write side) against reads (`upload`/`who`/`quota` take the read
//! side), and [`upload_slot`] additionally serializes `upload` invocations
//! because each one writes the shared upload database.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, warn};

use crate::database::models::ToolCredentialDbModel;
use crate::database::repositories::ToolCredentialRepository;
use crate::notification::{NotificationEvent, NotificationService};
use process_utils::NoWindowExt;

/// Binary name used when neither an explicit path nor [`BINARY_PATH_ENV`]
/// provides one; resolved through `PATH` at spawn time.
pub const DEFAULT_BINARY: &str = "BaiduPCS-Go";

/// Environment variable overriding the BaiduPCS-Go binary path, mirroring
/// how `RcloneProcessor` reads `RCLONE_PATH`.
pub const BINARY_PATH_ENV: &str = "BAIDUPCS_PATH";

/// Environment variable BaiduPCS-Go itself reads to locate its config
/// directory (login session, upload database). Set on child processes when
/// a config dir override is configured.
pub const CONFIG_DIR_ENV: &str = "BAIDUPCS_GO_CONFIG_DIR";

/// Marker printed by `BaiduPCS-Go login` on success
/// (`百度帐号登录成功: <name>`). The login command exits 0 even when setup
/// fails, so success must be detected from this marker, not the exit code.
pub const LOGIN_SUCCESS_MARKER: &str = "百度帐号登录成功";

/// Marker printed by `BaiduPCS-Go logout` after the account was removed
/// (`退出用户成功, <name>`).
pub const LOGOUT_SUCCESS_MARKER: &str = "退出用户成功";

/// Timeout for `BaiduPCS-Go login`, which performs network calls against
/// Baidu before persisting the session. Shared by the API login endpoint
/// and [`StoredLoginAuthenticator::relogin`].
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Resolve the BaiduPCS-Go binary path: explicit (per-job or per-request)
/// value first, then [`BINARY_PATH_ENV`], then [`DEFAULT_BINARY`].
pub fn resolve_binary_path(explicit: Option<&str>) -> String {
    if let Some(path) = explicit.map(str::trim).filter(|p| !p.is_empty()) {
        return path.to_string();
    }
    std::env::var(BINARY_PATH_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.to_string())
}

/// Process-wide lock over the BaiduPCS-Go config directory. `login`/`logout`
/// rewrite the session store and must hold the write side; uploads and
/// status probes hold the read side so they never observe a half-written
/// session. API handlers should use `try_write()` and reject with a
/// conflict instead of queueing behind a multi-hour upload.
pub fn cli_lock() -> &'static RwLock<()> {
    static LOCK: OnceLock<RwLock<()>> = OnceLock::new();
    LOCK.get_or_init(|| RwLock::new(()))
}

/// Single-permit semaphore serializing `upload` invocations across IO
/// workers: concurrent BaiduPCS-Go processes would share the upload
/// database in the config directory, which is a single-writer store.
pub fn upload_slot() -> &'static Semaphore {
    static SLOT: OnceLock<Semaphore> = OnceLock::new();
    SLOT.get_or_init(|| Semaphore::new(1))
}

/// Build a [`Command`] for the given binary with [`CONFIG_DIR_ENV`] applied
/// when `config_dir` is a non-empty override.
pub fn base_command(binary_path: &str, config_dir: Option<&str>) -> Command {
    let mut command = Command::new(binary_path);
    if let Some(dir) = config_dir.map(str::trim).filter(|d| !d.is_empty()) {
        command.env(CONFIG_DIR_ENV, dir);
    }
    command
}

/// Replace each secret value occurring in `text` with `***`. Applied to CLI
/// output before it is returned in API responses or logged, because the
/// login flow passes credentials on the command line and BaiduPCS-Go may
/// echo parts of its input on errors.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.is_empty() {
            continue;
        }
        out = out.replace(secret, "***");
    }
    out
}

/// Captured output of a one-shot CLI invocation run via [`run_capture`].
#[derive(Debug)]
pub struct CliOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    /// Stdout followed by stderr, for marker searches that don't care which
    /// stream a message was printed on.
    pub fn combined(&self) -> String {
        let mut combined = self.stdout.clone();
        if !self.stderr.trim().is_empty() {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str(&self.stderr);
        }
        combined
    }
}

/// Run a short-lived BaiduPCS-Go command (`login`, `who`, `quota`, `-v`,
/// `logout`) to completion, optionally writing `stdin_input` first (the
/// `logout` confirmation prompt reads `y\n` from stdin). The child is
/// killed when `timeout` elapses or the future is dropped
/// (`kill_on_drop`), so a surprise interactive prompt cannot hang a worker.
///
/// Long-running `upload` invocations do not use this helper; they stream
/// through `pipeline::processors::utils::run_baidupcs_with_logs` instead.
pub async fn run_capture(
    mut command: Command,
    stdin_input: Option<&[u8]>,
    timeout: Duration,
) -> crate::Result<CliOutput> {
    command.no_window();
    command.kill_on_drop(true);
    command.stdin(if stdin_input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn BaiduPCS-Go: {e}")))?;

    if let Some(bytes) = stdin_input
        && let Some(mut stdin) = child.stdin.take()
    {
        // Write errors are ignored: the child may exit before reading (e.g.
        // logout with no logged-in account never shows the prompt).
        let _ = stdin.write_all(bytes).await;
        let _ = stdin.shutdown().await;
    }

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(CliOutput {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(crate::Error::Other(format!(
            "Failed to wait for BaiduPCS-Go: {e}"
        ))),
        Err(_) => Err(crate::Error::Other(format!(
            "BaiduPCS-Go did not finish within {}s",
            timeout.as_secs()
        ))),
    }
}

/// Account identity parsed from `BaiduPCS-Go who` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaiduAccount {
    pub uid: u64,
    pub username: String,
}

/// Parse `who` output (`当前帐号 uid: %d, 用户名: %s, 性别: %s, 年龄: %.1f`).
/// A `uid` of 0 means no account is logged in; callers decide how to
/// surface that. Returns `None` when the line shape is unrecognized so the
/// raw output can be shown as a fallback detail.
pub fn parse_who(output: &str) -> Option<BaiduAccount> {
    for line in output.lines() {
        let Some(rest) = line.split("uid: ").nth(1) else {
            continue;
        };
        let uid_digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(uid) = uid_digits.parse::<u64>() else {
            continue;
        };
        let username = rest
            .split("用户名: ")
            .nth(1)
            .map(|name| name.split(", 性别").next().unwrap_or(name).trim().to_string())
            .unwrap_or_default();
        return Some(BaiduAccount { uid, username });
    }
    None
}

/// Storage usage parsed from `BaiduPCS-Go quota` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaiduQuota {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// Parse `quota` output
/// (`用户名: %s, 总空间: %s, 已用空间: %s, 比率: %f%%`), where sizes are
/// formatted by BaiduPCS-Go's `ConvertFileSize` (see [`parse_size`]).
pub fn parse_quota(output: &str) -> Option<BaiduQuota> {
    for line in output.lines() {
        let Some(total_part) = line.split("总空间: ").nth(1) else {
            continue;
        };
        let Some(total_bytes) = total_part.split(',').next().and_then(parse_size) else {
            continue;
        };
        let Some(used_bytes) = line
            .split("已用空间: ")
            .nth(1)
            .and_then(|part| part.split(',').next())
            .and_then(parse_size)
        else {
            continue;
        };
        return Some(BaiduQuota {
            used_bytes,
            total_bytes,
        });
    }
    None
}

/// Parse a size in BaiduPCS-Go's `ConvertFileSize` format: an integer byte
/// count (`123B`) or a decimal with a binary-multiple unit and no space
/// (`1.234567GB`, `2.000000TB`).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let unit_start = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = s.split_at(unit_start);
    let value: f64 = number.parse().ok()?;
    let multiplier: f64 = match unit.trim().to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" | "KIB" => 1024.0,
        "MB" | "MIB" => 1024f64.powi(2),
        "GB" | "GIB" => 1024f64.powi(3),
        "TB" | "TIB" => 1024f64.powi(4),
        "PB" | "PIB" => 1024f64.powi(5),
        _ => return None,
    };
    Some((value * multiplier).max(0.0) as u64)
}

/// Storage key for one BaiduPCS-Go session: the literal (trimmed) config
/// directory string, `""` for the CLI's default location. Used as
/// `tool_credentials.account_key` under [`CREDENTIAL_TOOL`].
pub fn config_dir_key(config_dir: Option<&str>) -> String {
    config_dir.map(str::trim).unwrap_or_default().to_string()
}

/// `tool_credentials.tool` value for this integration.
pub const CREDENTIAL_TOOL: &str = "baidupcs";

/// Login material in one of the two forms `BaiduPCS-Go login` accepts:
/// a full cookie string, or BDUSS with an optional STOKEN.
///
/// Serialized as the `tool_credentials.payload` JSON for
/// [`CREDENTIAL_TOOL`]; `#[serde(default)]` keeps older payloads loadable
/// if a field is ever added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoginMaterial {
    pub cookies: Option<String>,
    pub bduss: Option<String>,
    pub stoken: Option<String>,
}

impl LoginMaterial {
    fn field(value: Option<&String>) -> Option<&str> {
        value.map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    /// True when at least one accepted login form is present.
    pub fn is_usable(&self) -> bool {
        Self::field(self.cookies.as_ref()).is_some() || Self::field(self.bduss.as_ref()).is_some()
    }

    /// Arguments after `login`: `-cookies=` wins over `-bduss=`/`-stoken=`,
    /// matching the precedence of `BaiduPcsLoginRequest`. `None` when no
    /// usable material is present.
    pub fn login_args(&self) -> Option<Vec<String>> {
        if let Some(cookies) = Self::field(self.cookies.as_ref()) {
            return Some(vec![format!("-cookies={cookies}")]);
        }
        let bduss = Self::field(self.bduss.as_ref())?;
        let mut args = vec![format!("-bduss={bduss}")];
        if let Some(stoken) = Self::field(self.stoken.as_ref()) {
            args.push(format!("-stoken={stoken}"));
        }
        Some(args)
    }

    /// Values [`scrub`] must mask out of any relayed CLI output.
    pub fn secrets(&self) -> Vec<&str> {
        [&self.cookies, &self.bduss, &self.stoken]
            .into_iter()
            .filter_map(|value| Self::field(value.as_ref()))
            .collect()
    }
}

impl LoginMaterial {
    /// Parse a `tool_credentials.payload` JSON blob. A malformed payload
    /// reads as "no usable material" rather than an error, so a hand-edited
    /// row only disables auto re-login.
    fn from_payload(payload: &str) -> Option<Self> {
        match serde_json::from_str::<Self>(payload) {
            Ok(material) if material.is_usable() => Some(material),
            Ok(_) => None,
            Err(e) => {
                warn!(error = %e, "Stored BaiduPCS-Go credential payload is not readable");
                None
            }
        }
    }
}

/// Result of one `login` run: the CLI exits 0 even on failure, so
/// `success` comes from [`LOGIN_SUCCESS_MARKER`] and `message` carries the
/// scrubbed combined output for diagnostics.
#[derive(Debug)]
pub struct LoginOutcome {
    pub success: bool,
    pub message: String,
}

/// Run `BaiduPCS-Go login` with `material`. The caller must hold the write
/// side of [`cli_lock`] — this function does not lock, because the API
/// handler acquires `try_write` up front to reject with a conflict instead
/// of queueing. All errors and output are scrubbed of the material's
/// secret values before they leave this function.
pub async fn run_login(
    binary_path: &str,
    config_dir: Option<&str>,
    material: &LoginMaterial,
    timeout: Duration,
) -> crate::Result<LoginOutcome> {
    let args = material.login_args().ok_or_else(|| {
        crate::Error::Validation("No usable BaiduPCS-Go login material".to_string())
    })?;
    let secrets = material.secrets();

    let mut command = base_command(binary_path, config_dir);
    command.arg("login");
    command.args(&args);

    let output = run_capture(command, None, timeout)
        .await
        .map_err(|e| crate::Error::Other(scrub(&e.to_string(), &secrets)))?;
    let combined = output.combined();
    Ok(LoginOutcome {
        success: combined.contains(LOGIN_SUCCESS_MARKER),
        message: scrub(&combined, &secrets),
    })
}

/// How long a config-dir key sits out of automatic re-login after a
/// rejected or errored attempt. Bounds the failed-Baidu-call rate when the
/// stored material is permanently dead (e.g. password change): at most one
/// replay per window instead of one per upload job.
const RELOGIN_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// Failed re-login bookkeeping, keyed by [`config_dir_key`].
/// In-process only: a restart retries immediately, which is the desired
/// "maybe it works now" behavior after an operator intervenes.
struct ReloginCooldown {
    window: Duration,
    last_failure: Mutex<HashMap<String, Instant>>,
}

impl ReloginCooldown {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_failure: Mutex::new(HashMap::new()),
        }
    }

    fn should_attempt(&self, key: &str) -> bool {
        self.should_attempt_at(key, Instant::now())
    }

    fn should_attempt_at(&self, key: &str, now: Instant) -> bool {
        self.last_failure
            .lock()
            .unwrap()
            .get(key)
            .is_none_or(|failed_at| now.duration_since(*failed_at) >= self.window)
    }

    fn record_failure(&self, key: &str) {
        self.record_failure_at(key, Instant::now());
    }

    fn record_failure_at(&self, key: &str, now: Instant) {
        self.last_failure.lock().unwrap().insert(key.to_string(), now);
    }

    /// Cleared on success and whenever the stored material changes
    /// (`save_credentials` / `delete_credentials`), so fresh credentials are
    /// tried immediately instead of sitting out a stale window.
    fn clear(&self, key: &str) {
        self.last_failure.lock().unwrap().remove(key);
    }
}

/// Stored-credential operations shared by the API routes (save on
/// remembered login, delete on logout, `has_stored_credentials`) and by
/// `BaiduPcsProcessor` (automatic re-login when a session is logged out or
/// an upload attempt failed).
#[async_trait]
pub trait BaiduPcsAuthenticator: Send + Sync {
    /// Usable stored material exists for this config dir. Lookup errors
    /// degrade to `false` so a DB hiccup only skips auto re-login instead
    /// of failing the caller.
    async fn has_credentials(&self, config_dir: Option<&str>) -> bool;

    /// Persist login material for this config dir, replacing any earlier
    /// row.
    async fn save_credentials(
        &self,
        config_dir: Option<&str>,
        material: &LoginMaterial,
    ) -> crate::Result<()>;

    /// Forget the stored material for this config dir (no-op when absent).
    async fn delete_credentials(&self, config_dir: Option<&str>) -> crate::Result<()>;

    /// Replay the stored material through `BaiduPCS-Go login`. Returns
    /// `Ok(false)` when nothing usable is stored or the CLI rejected the
    /// material. Acquires the write side of [`cli_lock`] itself, so the
    /// caller must not hold either side.
    async fn relogin(&self, binary_path: &str, config_dir: Option<&str>) -> crate::Result<bool>;
}

/// Production [`BaiduPcsAuthenticator`] over the `tool_credentials` table
/// (rows scoped by [`CREDENTIAL_TOOL`]), with a per-config-dir failure
/// cooldown and a notification on rejected re-logins.
pub struct StoredLoginAuthenticator {
    repository: Arc<dyn ToolCredentialRepository>,
    cooldown: ReloginCooldown,
    /// Weak so the authenticator (held in a process-wide `OnceLock`) never
    /// keeps the notification service alive through shutdown; same shape
    /// as `crate::metrics::gpu_health`.
    notification_service: Weak<NotificationService>,
}

impl StoredLoginAuthenticator {
    pub fn new(repository: Arc<dyn ToolCredentialRepository>) -> Self {
        Self {
            repository,
            cooldown: ReloginCooldown::new(RELOGIN_COOLDOWN),
            notification_service: Weak::new(),
        }
    }

    /// Attach the notification service used to report rejected re-logins.
    pub fn with_notification_service(mut self, service: Weak<NotificationService>) -> Self {
        self.notification_service = service;
        self
    }

    /// Emit `NotificationEvent::BaiduPcsReloginFailed`. Called only after a
    /// real re-login attempt (never for cooldown skips), so with
    /// [`RELOGIN_COOLDOWN`] this fires at most once per window per key.
    fn notify_relogin_failure(&self, key: &str, message: &str) {
        let Some(service) = self.notification_service.upgrade() else {
            return;
        };
        service.dispatch_notification(NotificationEvent::BaiduPcsReloginFailed {
            config_dir: if key.is_empty() {
                "default".to_string()
            } else {
                key.to_string()
            },
            message: truncate_chars(message, 300),
            timestamp: chrono::Utc::now(),
        });
    }
}

/// First `max` characters of trimmed `text`, for relaying CLI output into
/// notification payloads.
fn truncate_chars(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

#[async_trait]
impl BaiduPcsAuthenticator for StoredLoginAuthenticator {
    async fn has_credentials(&self, config_dir: Option<&str>) -> bool {
        let key = config_dir_key(config_dir);
        match self.repository.get(CREDENTIAL_TOOL, &key).await {
            Ok(Some(row)) => LoginMaterial::from_payload(&row.payload).is_some(),
            Ok(None) => false,
            Err(e) => {
                warn!(error = %e, "Failed to look up stored BaiduPCS-Go credentials");
                false
            }
        }
    }

    async fn save_credentials(
        &self,
        config_dir: Option<&str>,
        material: &LoginMaterial,
    ) -> crate::Result<()> {
        let key = config_dir_key(config_dir);
        let payload = serde_json::to_string(material).map_err(|e| {
            crate::Error::Other(format!("Failed to serialize BaiduPCS-Go credentials: {e}"))
        })?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.repository
            .upsert(&ToolCredentialDbModel {
                tool: CREDENTIAL_TOOL.to_string(),
                account_key: key.clone(),
                payload,
                created_at: now_ms,
                updated_at: now_ms,
            })
            .await?;
        // Fresh material must be tried immediately, not sit out a cooldown
        // earned by the previous credentials.
        self.cooldown.clear(&key);
        Ok(())
    }

    async fn delete_credentials(&self, config_dir: Option<&str>) -> crate::Result<()> {
        let key = config_dir_key(config_dir);
        self.repository.delete(CREDENTIAL_TOOL, &key).await?;
        self.cooldown.clear(&key);
        Ok(())
    }

    async fn relogin(&self, binary_path: &str, config_dir: Option<&str>) -> crate::Result<bool> {
        let key = config_dir_key(config_dir);
        if !self.cooldown.should_attempt(&key) {
            debug!("Skipping BaiduPCS-Go re-login during failure cooldown");
            return Ok(false);
        }
        let Some(row) = self.repository.get(CREDENTIAL_TOOL, &key).await? else {
            return Ok(false);
        };
        let Some(material) = LoginMaterial::from_payload(&row.payload) else {
            return Ok(false);
        };

        let outcome = {
            let _guard = cli_lock().write().await;
            run_login(binary_path, config_dir, &material, LOGIN_TIMEOUT).await
        };
        match outcome {
            Ok(outcome) if outcome.success => {
                self.cooldown.clear(&key);
                Ok(true)
            }
            Ok(outcome) => {
                self.cooldown.record_failure(&key);
                warn!(
                    message = %outcome.message,
                    "Automatic BaiduPCS-Go re-login was rejected; stored credentials may have expired"
                );
                self.notify_relogin_failure(&key, &outcome.message);
                Ok(false)
            }
            Err(e) => {
                self.cooldown.record_failure(&key);
                self.notify_relogin_failure(&key, &e.to_string());
                Err(e)
            }
        }
    }
}

/// Process-wide authenticator installed once at startup by the service
/// container (`ServiceContainerBuilder`), after the database pools exist.
/// `None` (e.g. in unit tests) disables stored-credential features; callers
/// must degrade gracefully.
pub fn set_authenticator(authenticator: Arc<dyn BaiduPcsAuthenticator>) {
    let _ = AUTHENTICATOR.set(authenticator);
}

/// The installed authenticator, if any.
pub fn authenticator() -> Option<Arc<dyn BaiduPcsAuthenticator>> {
    AUTHENTICATOR.get().cloned()
}

static AUTHENTICATOR: OnceLock<Arc<dyn BaiduPcsAuthenticator>> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_who_extracts_uid_and_username() {
        let output = "当前帐号 uid: 1234567, 用户名: some_user, 性别: male, 年龄: 25.0\n";
        assert_eq!(
            parse_who(output),
            Some(BaiduAccount {
                uid: 1234567,
                username: "some_user".to_string(),
            })
        );
    }

    #[test]
    fn parse_who_reports_uid_zero_when_logged_out() {
        let output = "当前帐号 uid: 0, 用户名: , 性别: unknown, 年龄: 0.0";
        let account = parse_who(output).expect("line shape still parses");
        assert_eq!(account.uid, 0);
        assert_eq!(account.username, "");
    }

    #[test]
    fn parse_who_rejects_unrelated_output() {
        assert_eq!(parse_who("未登录"), None);
        assert_eq!(parse_who(""), None);
    }

    #[test]
    fn parse_quota_extracts_sizes() {
        let output = "用户名: some_user, 总空间: 2.000000TB, 已用空间: 512.000000GB, 比率: 25.000000%\n";
        let quota = parse_quota(output).expect("quota line parses");
        assert_eq!(quota.total_bytes, 2 * 1024u64.pow(4));
        assert_eq!(quota.used_bytes, 512 * 1024u64.pow(3));
    }

    #[test]
    fn parse_quota_rejects_error_output() {
        assert_eq!(parse_quota("获取当前用户空间配额信息, 发生错误"), None);
    }

    #[test]
    fn parse_size_handles_convert_file_size_formats() {
        assert_eq!(parse_size("123B"), Some(123));
        assert_eq!(parse_size("1.000000KB"), Some(1024));
        assert_eq!(parse_size("1.500000MB"), Some(1_572_864));
        assert_eq!(parse_size("2.000000TB"), Some(2 * 1024u64.pow(4)));
        assert_eq!(parse_size(" 1.000000GB "), Some(1024u64.pow(3)));
        assert_eq!(parse_size("weird"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn scrub_masks_all_secret_occurrences() {
        let text = "login -bduss=SECRETVALUE failed; SECRETVALUE rejected";
        assert_eq!(
            scrub(text, &["SECRETVALUE", "", "  "]),
            "login -bduss=*** failed; *** rejected"
        );
    }

    #[test]
    fn resolve_binary_path_prefers_explicit_value() {
        assert_eq!(resolve_binary_path(Some(" /opt/bpcs ")), "/opt/bpcs");
        assert_eq!(resolve_binary_path(Some("   ")), resolve_binary_path(None));
    }

    #[test]
    fn config_dir_key_normalizes_blank_to_default() {
        assert_eq!(config_dir_key(None), "");
        assert_eq!(config_dir_key(Some("   ")), "");
        assert_eq!(config_dir_key(Some(" /cfg ")), "/cfg");
    }

    #[test]
    fn login_material_prefers_cookies_over_bduss() {
        let material = LoginMaterial {
            cookies: Some("BDUSS=a; STOKEN=B".to_string()),
            bduss: Some("ignored".to_string()),
            stoken: Some("ignored".to_string()),
        };
        assert!(material.is_usable());
        assert_eq!(
            material.login_args(),
            Some(vec!["-cookies=BDUSS=a; STOKEN=B".to_string()])
        );
    }

    #[test]
    fn login_material_bduss_form_and_unusable() {
        let material = LoginMaterial {
            cookies: None,
            bduss: Some("bd".to_string()),
            stoken: Some("ST".to_string()),
        };
        assert_eq!(
            material.login_args(),
            Some(vec!["-bduss=bd".to_string(), "-stoken=ST".to_string()])
        );
        assert_eq!(material.secrets(), vec!["bd", "ST"]);

        let empty = LoginMaterial {
            cookies: Some("  ".to_string()),
            bduss: None,
            stoken: Some("orphan-stoken".to_string()),
        };
        assert!(!empty.is_usable());
        assert_eq!(empty.login_args(), None);
    }

    #[test]
    fn relogin_cooldown_blocks_within_window_only() {
        let cooldown = ReloginCooldown::new(Duration::from_secs(3600));
        let start = Instant::now();

        assert!(cooldown.should_attempt_at("", start));
        cooldown.record_failure_at("", start);
        assert!(!cooldown.should_attempt_at("", start + Duration::from_secs(3599)));
        assert!(cooldown.should_attempt_at("", start + Duration::from_secs(3600)));
        // Other keys are unaffected.
        assert!(cooldown.should_attempt_at("/other", start));

        cooldown.record_failure_at("", start);
        cooldown.clear("");
        assert!(cooldown.should_attempt_at("", start));
    }

    struct MockCredentialRepo {
        row: Option<ToolCredentialDbModel>,
        gets: std::sync::atomic::AtomicUsize,
    }

    impl MockCredentialRepo {
        fn new(row: Option<ToolCredentialDbModel>) -> Arc<Self> {
            Arc::new(Self {
                row,
                gets: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn gets(&self) -> usize {
            self.gets.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl ToolCredentialRepository for MockCredentialRepo {
        async fn get(
            &self,
            tool: &str,
            account_key: &str,
        ) -> crate::Result<Option<ToolCredentialDbModel>> {
            self.gets
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self
                .row
                .clone()
                .filter(|row| row.tool == tool && row.account_key == account_key))
        }

        async fn upsert(&self, _model: &ToolCredentialDbModel) -> crate::Result<()> {
            Ok(())
        }

        async fn delete(&self, _tool: &str, _account_key: &str) -> crate::Result<()> {
            Ok(())
        }
    }

    fn stored_row() -> ToolCredentialDbModel {
        ToolCredentialDbModel {
            tool: CREDENTIAL_TOOL.to_string(),
            account_key: String::new(),
            payload: r#"{"cookies":"BDUSS=abc; STOKEN=DEF"}"#.to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn relogin_skips_everything_during_cooldown() {
        let repo = MockCredentialRepo::new(Some(stored_row()));
        let authenticator = StoredLoginAuthenticator::new(repo.clone());
        authenticator.cooldown.record_failure("");

        let result = authenticator.relogin("BaiduPCS-Go", None).await.unwrap();

        assert!(!result);
        assert_eq!(repo.gets(), 0, "cooldown short-circuits before the store");
    }

    #[tokio::test]
    async fn relogin_without_stored_material_records_no_failure() {
        let repo = MockCredentialRepo::new(None);
        let authenticator = StoredLoginAuthenticator::new(repo.clone());

        let result = authenticator.relogin("BaiduPCS-Go", None).await.unwrap();

        assert!(!result);
        assert_eq!(repo.gets(), 1);
        // Nothing was attempted, so nothing entered cooldown.
        assert!(authenticator.cooldown.should_attempt(""));
    }

    #[tokio::test]
    async fn saving_credentials_clears_the_cooldown() {
        let repo = MockCredentialRepo::new(None);
        let authenticator = StoredLoginAuthenticator::new(repo);
        authenticator.cooldown.record_failure("");
        assert!(!authenticator.cooldown.should_attempt(""));

        authenticator
            .save_credentials(
                None,
                &LoginMaterial {
                    cookies: Some("BDUSS=new; STOKEN=NEW".to_string()),
                    bduss: None,
                    stoken: None,
                },
            )
            .await
            .unwrap();

        assert!(authenticator.cooldown.should_attempt(""));
    }
}
