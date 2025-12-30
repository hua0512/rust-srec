//! TDL (Telegram Downloader) integration routes.
//!
//! These endpoints provide a best-effort "remote interactive" wrapper around `tdl login`.
//! The `tdl` CLI is interactive and persists its session on disk; pipeline jobs should rely on
//! an already-initialized session directory.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

const MAX_OUTPUT_CHUNKS: usize = 200;
const DEFAULT_SESSION_TTL_SECS: u64 = 15 * 60;
const DEFAULT_SENSITIVE_OUTPUT_SUPPRESS_SECS: u64 = 5;

fn detect_password_prompt(chunk: &str) -> bool {
    let lower = chunk.to_ascii_lowercase();
    // Best-effort detection of interactive 2FA/password prompts.
    lower.contains("password")
        || lower.contains("two-factor")
        || lower.contains("2fa")
        || lower.contains("cloud password")
        || lower.contains("2-step")
        || chunk.contains("密码")
        || chunk.contains("口令")
        || chunk.contains("两步验证")
        || chunk.contains("二步验证")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TdlLoginStatus {
    Running,
    Exited { code: Option<i32> },
    Failed { message: String },
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TdlLoginState {
    LoggedIn,
    NotLoggedIn,
    Unknown,
}

#[derive(Debug)]
struct TdlLoginSession {
    created_at: std::time::Instant,
    ttl: std::time::Duration,
    allow_password: bool,
    sensitive_output_suppress_secs: u64,
    suppress_output_until: parking_lot::Mutex<Option<std::time::Instant>>,
    status: parking_lot::RwLock<TdlLoginStatus>,
    output: parking_lot::Mutex<VecDeque<String>>,
    input_tx: mpsc::Sender<String>,
    cancel: CancellationToken,
}

impl TdlLoginSession {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }

    fn push_output(&self, chunk: String) {
        let mut out = self.output.lock();
        if out.len() >= MAX_OUTPUT_CHUNKS {
            out.pop_front();
        }
        out.push_back(chunk);
    }
}

#[derive(Default)]
struct TdlLoginManager {
    sessions: DashMap<String, Arc<TdlLoginSession>>,
}

impl TdlLoginManager {
    fn get() -> &'static Arc<TdlLoginManager> {
        static INSTANCE: OnceLock<Arc<TdlLoginManager>> = OnceLock::new();
        INSTANCE.get_or_init(|| Arc::new(TdlLoginManager::default()))
    }

    fn prune(&self) {
        let expired: Vec<(String, Arc<TdlLoginSession>)> = self
            .sessions
            .iter()
            .filter(|e| e.value().is_expired())
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        for (id, session) in expired {
            session.cancel.cancel();
            self.sessions.remove(&id);
        }
    }
}

/// Request to start a `tdl login` session.
#[derive(Debug, Clone, Deserialize)]
pub struct StartLoginRequest {
    /// Optional override for the `tdl` binary path. If omitted, uses `TDL_PATH`, then `tdl`.
    pub tdl_path: Option<String>,
    /// Optional working directory for `tdl` (useful to pin session/config storage).
    pub working_dir: Option<String>,
    /// Extra env vars for `tdl` (e.g. HOME/XDG_* overrides).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Global args inserted before the `login` subcommand (e.g. `--storage ...`, `--ns ...`).
    #[serde(default)]
    pub global_args: Vec<String>,
    /// Optional TTL in seconds for this interactive session (default: 15 minutes).
    pub ttl_secs: Option<u64>,
    /// Allow Telegram 2FA password input via this API session.
    ///
    /// When disabled (default), if `tdl` prompts for a password/2FA secret the login session is
    /// immediately aborted to avoid handling sensitive secrets via HTTP.
    #[serde(default)]
    pub allow_password: bool,
    /// When sending sensitive input (like 2FA password), suppress captured output briefly to
    /// reduce the risk of echoing secrets back in stdout/stderr (some CLIs echo when not in a TTY).
    #[serde(default)]
    pub suppress_output_on_sensitive_input_secs: Option<u64>,
    /// Additional args appended after `login` (rarely needed; kept for flexibility).
    #[serde(default)]
    pub login_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartLoginResponse {
    pub session_id: String,
    pub status: TdlLoginStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendInputRequest {
    /// A single line of input to send (newline is appended server-side).
    pub text: String,
    /// Marks this input as sensitive (e.g., 2FA password). When true, output is temporarily
    /// suppressed to reduce the chance of echoing secrets back to callers.
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginStatusResponse {
    pub session_id: String,
    pub status: TdlLoginStatus,
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TdlStatusRequest {
    /// Optional override for the `tdl` binary path. If omitted, uses `TDL_PATH`, then `tdl`.
    pub tdl_path: Option<String>,
    /// Optional working directory for `tdl` (useful to pin session/config storage).
    pub working_dir: Option<String>,
    /// Extra env vars for `tdl` (e.g. HOME/XDG_* overrides).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Global args inserted before subcommands (e.g. `--storage ...`, `--ns ...`).
    #[serde(default)]
    pub global_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TdlStatusResponse {
    pub resolved_tdl_path: String,
    pub binary_ok: bool,
    pub version: Option<String>,
    pub login_state: TdlLoginState,
    pub detail: Option<String>,
}

fn resolve_tdl_path(requested: Option<String>) -> String {
    requested
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("TDL_PATH").ok())
        .unwrap_or_else(|| "tdl".to_string())
}

async fn spawn_tdl_login(
    session: Arc<TdlLoginSession>,
    tdl_path: String,
    working_dir: Option<String>,
    env: HashMap<String, String>,
    global_args: Vec<String>,
    login_args: Vec<String>,
    mut input_rx: mpsc::Receiver<String>,
) {
    let mut command = Command::new(&tdl_path);
    if !global_args.is_empty() {
        command.args(global_args);
    }
    command.arg("login");
    if !login_args.is_empty() {
        command.args(login_args);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = working_dir.as_deref()
        && !dir.trim().is_empty()
    {
        command.current_dir(dir);
    }
    if !env.is_empty() {
        command.envs(env.iter());
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            *session.status.write() = TdlLoginStatus::Failed {
                message: format!("Failed to start tdl: {}", e),
            };
            return;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(v) => v,
        None => {
            *session.status.write() = TdlLoginStatus::Failed {
                message: "tdl stdin unavailable".to_string(),
            };
            return;
        }
    };

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let cancel = session.cancel.clone();

    // Forward API inputs to process stdin, without logging contents.
    let stdin_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                msg = input_rx.recv() => {
                    let Some(line) = msg else { break };
                    if stdin.write_all(line.as_bytes()).await.is_err() { break; }
                    if stdin.write_all(b"\n").await.is_err() { break; }
                    let _ = stdin.flush().await;
                }
            }
        }
    });

    // Capture stdout/stderr as chunks (prompts may not be newline-terminated).
    let session_out = session.clone();
    let stdout_task = tokio::spawn(async move {
        let Some(mut stdout) = stdout.take() else {
            return;
        };
        let mut buf = vec![0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(until) = *session_out.suppress_output_until.lock()
                        && std::time::Instant::now() < until
                    {
                        continue;
                    }
                    if !session_out.allow_password && detect_password_prompt(&chunk) {
                        *session_out.status.write() = TdlLoginStatus::Failed {
                            message: "TDL login requires a password/2FA secret, which is disabled for this API session.".to_string(),
                        };
                        session_out.cancel.cancel();
                        session_out.push_output(chunk);
                        break;
                    }
                    session_out.push_output(chunk);
                }
                Err(_) => break,
            }
        }
    });

    let session_err = session.clone();
    let stderr_task = tokio::spawn(async move {
        let Some(mut stderr) = stderr.take() else {
            return;
        };
        let mut buf = vec![0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(until) = *session_err.suppress_output_until.lock()
                        && std::time::Instant::now() < until
                    {
                        continue;
                    }
                    if !session_err.allow_password && detect_password_prompt(&chunk) {
                        *session_err.status.write() = TdlLoginStatus::Failed {
                            message: "TDL login requires a password/2FA secret, which is disabled for this API session.".to_string(),
                        };
                        session_err.cancel.cancel();
                        session_err.push_output(chunk);
                        break;
                    }
                    session_err.push_output(chunk);
                }
                Err(_) => break,
            }
        }
    });

    // Wait for process exit or cancellation.
    tokio::select! {
        _ = session.cancel.cancelled() => {
            let _ = child.kill().await;
            // Only mark as cancelled if nothing else already set a more specific terminal status.
            if matches!(&*session.status.read(), TdlLoginStatus::Running) {
                *session.status.write() = TdlLoginStatus::Cancelled;
            }
        }
        status = child.wait() => {
            match status {
                Ok(s) => {
                    if matches!(&*session.status.read(), TdlLoginStatus::Running) {
                        *session.status.write() = TdlLoginStatus::Exited { code: s.code() };
                    }
                }
                Err(e) => {
                    if matches!(&*session.status.read(), TdlLoginStatus::Running) {
                        *session.status.write() = TdlLoginStatus::Failed { message: format!("Failed to wait for tdl: {}", e) };
                    }
                }
            }
        }
    }

    // Best-effort: let background tasks drain.
    let _ = stdin_task.await;
    let _ = stdout_task.await;
    let _ = stderr_task.await;
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", post(get_tdl_status))
        .route("/login/start", post(start_login))
        .route("/login/{session_id}", get(get_login_status))
        .route("/login/{session_id}/input", post(send_login_input))
        .route("/login/{session_id}/cancel", post(cancel_login))
}

async fn run_tdl_output(
    tdl_path: &str,
    working_dir: Option<&str>,
    env: &HashMap<String, String>,
    global_args: &[String],
    args: &[&str],
) -> Result<std::process::Output, std::io::Error> {
    let mut cmd = Command::new(tdl_path);
    if !global_args.is_empty() {
        cmd.args(global_args);
    }
    cmd.args(args);
    if let Some(dir) = working_dir
        && !dir.trim().is_empty()
    {
        cmd.current_dir(dir);
    }
    if !env.is_empty() {
        cmd.envs(env.iter());
    }

    cmd.output().await
}

fn is_unknown_subcommand(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("unrecognized subcommand")
        || lower.contains("unknown command")
        || lower.contains("unknown subcommand")
        || lower.contains("found argument") && lower.contains("which wasn't expected")
}

fn is_not_logged_in(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("not logged")
        || lower.contains("not authorized")
        || lower.contains("unauthorized")
        || lower.contains("auth") && lower.contains("required")
        || text.contains("未登录")
        || text.contains("未登入")
        || text.contains("未授权")
        || text.contains("未授權")
}

pub async fn get_tdl_status(
    State(_state): State<AppState>,
    Json(payload): Json<TdlStatusRequest>,
) -> ApiResult<Json<TdlStatusResponse>> {
    let tdl_path = resolve_tdl_path(payload.tdl_path);
    let working_dir = payload.working_dir.as_deref();

    // Version check (best effort).
    let version_output = match timeout(
        std::time::Duration::from_secs(3),
        run_tdl_output(&tdl_path, working_dir, &payload.env, &payload.global_args, &["--version"]),
    )
    .await
    {
        Ok(Ok(out)) => Some(out),
        Ok(Err(e)) => {
            return Ok(Json(TdlStatusResponse {
                resolved_tdl_path: tdl_path,
                binary_ok: false,
                version: None,
                login_state: TdlLoginState::Unknown,
                detail: Some(format!("Failed to start tdl: {}", e)),
            }));
        }
        Err(_) => None,
    };

    let mut version: Option<String> = None;
    if let Some(out) = version_output.as_ref() {
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        if text.trim().is_empty() {
            text = String::from_utf8_lossy(&out.stderr).to_string();
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            version = Some(trimmed.to_string());
        }
    }

    // Login check (best effort): try a few common self-identification subcommands.
    let candidates: &[&[&str]] = &[&["me"], &["whoami"], &["account"], &["user"]];
    let mut last_detail: Option<String> = None;
    for args in candidates {
        let out = match timeout(
            std::time::Duration::from_secs(5),
            run_tdl_output(&tdl_path, working_dir, &payload.env, &payload.global_args, args),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Ok(Json(TdlStatusResponse {
                    resolved_tdl_path: tdl_path,
                    binary_ok: true,
                    version,
                    login_state: TdlLoginState::Unknown,
                    detail: Some(format!("Failed to run tdl {:?}: {}", args, e)),
                }));
            }
            Err(_) => {
                last_detail = Some(format!("Timed out running tdl {:?}", args));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let combined = format!("{}{}", stdout, stderr);

        if out.status.success() {
            let nonempty = combined.trim();
            if nonempty.is_empty() {
                last_detail = Some(format!("tdl {:?} returned success with no output", args));
                continue;
            }
            return Ok(Json(TdlStatusResponse {
                resolved_tdl_path: tdl_path,
                binary_ok: true,
                version,
                login_state: TdlLoginState::LoggedIn,
                detail: Some(nonempty.to_string()),
            }));
        }

        if is_unknown_subcommand(&stderr) {
            last_detail = Some(format!("tdl {:?} not supported: {}", args, stderr.trim()));
            continue;
        }

        if is_not_logged_in(&combined) {
            return Ok(Json(TdlStatusResponse {
                resolved_tdl_path: tdl_path,
                binary_ok: true,
                version,
                login_state: TdlLoginState::NotLoggedIn,
                detail: Some(combined.trim().to_string()),
            }));
        }

        last_detail = Some(format!(
            "tdl {:?} exited with {:?}: {}{}",
            args,
            out.status.code(),
            stdout.trim(),
            if stderr.trim().is_empty() {
                "".to_string()
            } else {
                format!("\n{}", stderr.trim())
            }
        ));
    }

    Ok(Json(TdlStatusResponse {
        resolved_tdl_path: tdl_path,
        binary_ok: true,
        version,
        login_state: TdlLoginState::Unknown,
        detail: last_detail,
    }))
}

pub async fn start_login(
    State(_state): State<AppState>,
    Json(payload): Json<StartLoginRequest>,
) -> ApiResult<Json<StartLoginResponse>> {
    let manager = TdlLoginManager::get().clone();
    manager.prune();

    let session_id = uuid::Uuid::new_v4().to_string();
    let ttl = std::time::Duration::from_secs(payload.ttl_secs.unwrap_or(DEFAULT_SESSION_TTL_SECS));

    let (input_tx, input_rx) = mpsc::channel::<String>(32);
    let suppress_secs = payload
        .suppress_output_on_sensitive_input_secs
        .unwrap_or(DEFAULT_SENSITIVE_OUTPUT_SUPPRESS_SECS)
        .clamp(1, 60);

    let session = Arc::new(TdlLoginSession {
        created_at: std::time::Instant::now(),
        ttl,
        allow_password: payload.allow_password,
        sensitive_output_suppress_secs: suppress_secs,
        suppress_output_until: parking_lot::Mutex::new(None),
        status: parking_lot::RwLock::new(TdlLoginStatus::Running),
        output: parking_lot::Mutex::new(VecDeque::new()),
        input_tx: input_tx.clone(),
        cancel: CancellationToken::new(),
    });

    // Store session.
    manager.sessions.insert(session_id.clone(), session.clone());

    // Spawn process task.
    let tdl_path = resolve_tdl_path(payload.tdl_path);
    let working_dir = payload.working_dir;
    let env = payload.env;
    let global_args = payload.global_args;
    let login_args = payload.login_args;

    let session_for_spawn = session.clone();
    tokio::spawn(async move {
        spawn_tdl_login(
            session_for_spawn,
            tdl_path,
            working_dir,
            env,
            global_args,
            login_args,
            input_rx,
        )
        .await;
    });

    // Initial output hint.
    session.push_output("Started `tdl login`. Use /input to send answers.\n".to_string());

    Ok(Json(StartLoginResponse {
        session_id,
        status: (*session.status.read()).clone(),
    }))
}

pub async fn get_login_status(
    State(_state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<LoginStatusResponse>> {
    let manager = TdlLoginManager::get();
    manager.prune();

    let session = manager
        .sessions
        .get(&session_id)
        .map(|e| e.value().clone())
        .ok_or_else(|| ApiError::not_found(format!("Login session {} not found", session_id)))?;

    let status = (*session.status.read()).clone();
    let output = session.output.lock().iter().cloned().collect();

    Ok(Json(LoginStatusResponse {
        session_id,
        status,
        output,
    }))
}

pub async fn send_login_input(
    State(_state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendInputRequest>,
) -> ApiResult<Json<()>> {
    let manager = TdlLoginManager::get();
    manager.prune();

    let session = manager
        .sessions
        .get(&session_id)
        .map(|e| e.value().clone())
        .ok_or_else(|| ApiError::not_found(format!("Login session {} not found", session_id)))?;

    match (*session.status.read()).clone() {
        TdlLoginStatus::Running => {}
        other => {
            return Err(ApiError::conflict(format!(
                "Login session is not running (status={:?})",
                other
            )));
        }
    }

    // Never log `payload.text`.
    if payload.sensitive {
        let suppress_secs = session.sensitive_output_suppress_secs.clamp(1, 60);
        *session.suppress_output_until.lock() =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(suppress_secs));
    }
    if session.input_tx.send(payload.text).await.is_err() {
        return Err(ApiError::internal("Failed to send input to tdl process"));
    }

    Ok(Json(()))
}

pub async fn cancel_login(
    State(_state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<()>> {
    let manager = TdlLoginManager::get();
    manager.prune();

    let session = manager
        .sessions
        .get(&session_id)
        .map(|e| e.value().clone())
        .ok_or_else(|| ApiError::not_found(format!("Login session {} not found", session_id)))?;

    session.cancel.cancel();
    Ok(Json(()))
}
