//! Shared helpers for invoking the BaiduPCS-Go CLI.
//!
//! Used by both `pipeline::processors::baidupcs` (uploads) and
//! `api::routes::baidupcs` (login/status/logout). BaiduPCS-Go persists its
//! login session and upload-state databases inside one config directory
//! (overridable via [`CONFIG_DIR_ENV`]), which is not safe under concurrent
//! writers: [`cli_lock`] serializes session mutation (`login`/`logout` take
//! the write side) against reads (`upload`/`who`/`quota` take the read
//! side). Each `upload` also writes the shared upload database, so the
//! pipeline runs uploads one at a time
//! ([`crate::pipeline::Processor::runs_one_at_a_time`]).

use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use process_utils::NoWindowExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

pub use crate::domain::processor::{BaiduPcsAuthenticator, LoginMaterial};
use crate::domain::proxy::ProxyTarget;

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
/// and [`BaiduPcsAuthenticator::relogin`].
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
pub fn cli_lock() -> &'static Arc<RwLock<()>> {
    static LOCK: OnceLock<Arc<RwLock<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(RwLock::new(())))
}

/// Build a [`Command`] for the given binary with [`CONFIG_DIR_ENV`] applied
/// when `config_dir` is a non-empty override, for commands that only read
/// the local config (`-v`, `env`, `who`, `logout`). They run without any
/// proxy variable, so no proxy login reaches them.
pub fn base_command(binary_path: &str, config_dir: Option<&str>) -> Command {
    let mut command = tool_command(binary_path, config_dir);
    ProxyTarget::Direct.apply_environment(&mut command);
    command
}

/// The binary with [`CONFIG_DIR_ENV`] applied, on the server's environment.
fn tool_command(binary_path: &str, config_dir: Option<&str>) -> Command {
    let mut command = Command::new(binary_path);
    if let Some(dir) = config_dir.map(str::trim).filter(|d| !d.is_empty()) {
        command.env(CONFIG_DIR_ENV, dir);
    }
    command
}

/// How long `BaiduPCS-Go env` gets to report the config directory.
const ENV_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// A [`base_command`] that reaches Baidu through `proxy`, for uploads and
/// quota checks. Commands run for an upload step take the step's route;
/// account commands from the tools page follow the global route.
///
/// BaiduPCS-Go prefers a proxy set in its own config over the environment,
/// so a route other than System proxy fails while one is set there.
pub async fn connected_command(
    binary_path: &str,
    config_dir: Option<&str>,
    proxy: &ProxyTarget,
) -> crate::Result<Command> {
    if *proxy != ProxyTarget::System {
        let mut probe = base_command(binary_path, config_dir);
        probe.arg("env");
        let output = run_capture(probe, None, ENV_PROBE_TIMEOUT).await?;
        let directory = login::config_directory_from(binary_path, output).await?;
        let proxy = proxy.clone();
        tokio::task::spawn_blocking(move || login::reject_own_proxy(&directory, &proxy))
            .await
            .map_err(|error| {
                crate::Error::Other(format!("BaiduPCS-Go config check task failed: {error}"))
            })??;
    }
    let mut command = tool_command(binary_path, config_dir);
    crate::domain::proxy::set_tool_environment(&mut command, proxy, "BaiduPCS-Go")?;
    Ok(command)
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
/// `logout` confirmation prompt reads `y\n` from stdin).
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
    process_utils::use_system_thp(&mut command);

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn BaiduPCS-Go: {e}")))?;

    if let Some(bytes) = stdin_input
        && let Some(mut stdin) = child.stdin.take()
    {
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
            .map(|name| {
                name.split(", 性别")
                    .next()
                    .unwrap_or(name)
                    .trim()
                    .to_string()
            })
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

/// Result of one `login` run: the CLI exits 0 even on failure, so
/// `success` comes from [`LOGIN_SUCCESS_MARKER`] and `message` carries the
/// scrubbed combined output for diagnostics.
#[derive(Debug)]
pub struct LoginOutcome {
    pub success: bool,
    pub message: String,
}

mod login;

/// Run a login through private stdin/config staging. The owned write-lock lease
/// remains held by cleanup if the caller disconnects before the child exits.
pub async fn run_login(
    binary_path: &str,
    config_dir: Option<&str>,
    proxy: &ProxyTarget,
    material: &LoginMaterial,
    timeout: Duration,
    lease: Arc<OwnedRwLockWriteGuard<()>>,
) -> crate::Result<LoginOutcome> {
    login::run(binary_path, config_dir, proxy, material, timeout, lease).await
}

/// Process-wide authenticator installed once at startup by the service
/// container, after the database pools exist. `None` (e.g. in unit tests)
/// disables stored-credential features; callers must degrade gracefully.
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
    fn parse_quota_extracts_sizes() {
        let output =
            "用户名: some_user, 总空间: 2.000000TB, 已用空间: 512.000000GB, 比率: 25.000000%\n";
        let quota = parse_quota(output).expect("quota line parses");
        assert_eq!(quota.total_bytes, 2 * 1024u64.pow(4));
        assert_eq!(quota.used_bytes, 512 * 1024u64.pow(3));
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
}
