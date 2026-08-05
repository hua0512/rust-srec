//! BaiduPCS-Go (Baidu Netdisk) tool routes.
//!
//! One-shot wrappers around the BaiduPCS-Go CLI so the web UI can log in
//! with pasted BDUSS+STOKEN or a cookie string, show the active account and
//! quota, and log out. Credentials flow one-way into the `login` argv and
//! persist only in BaiduPCS-Go's own config directory
//! (`crate::baidupcs::CONFIG_DIR_ENV`); rust-srec never stores them, and
//! every response passes relayed CLI output through `baidupcs::scrub` so
//! they are never echoed back. The CLI exits 0 even when login fails, so
//! success is detected from `baidupcs::LOGIN_SUCCESS_MARKER`.

use axum::{Json, Router, routing::post};
use std::time::Duration;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::baidupcs;

/// Per-probe timeout for `-v` / `who` / `quota`.
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// `logout` only prompts for confirmation and rewrites the local session.
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(15);

/// Longest CLI output tail relayed in responses.
const MAX_DETAIL_CHARS: usize = 500;

/// Create the BaiduPCS-Go tools router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", post(baidupcs_status))
        .route("/login", post(baidupcs_login))
        .route("/logout", post(baidupcs_logout))
}

/// Binary/config-dir overrides shared by the status and logout endpoints.
/// Both are optional; resolution falls back to `BAIDUPCS_PATH` and the
/// CLI's default config location.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct BaiduPcsToolRequest {
    pub binary_path: Option<String>,
    pub config_dir: Option<String>,
}

/// Result of probing the BaiduPCS-Go binary and login session.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct BaiduPcsStatusResponse {
    /// Binary path after `BAIDUPCS_PATH`/default resolution.
    pub resolved_binary_path: String,
    /// The binary spawned and reported a version.
    pub binary_ok: bool,
    pub version: Option<String>,
    /// `who` reported a non-zero uid.
    pub logged_in: bool,
    pub uid: Option<u64>,
    pub username: Option<String>,
    pub quota_used_bytes: Option<u64>,
    pub quota_total_bytes: Option<u64>,
    /// Login material is stored for this config dir, so the pipeline can
    /// re-login automatically. The material itself is never returned.
    pub has_stored_credentials: bool,
    /// Trimmed CLI output when a probe could not be parsed, so the UI can
    /// degrade to showing what the tool said.
    pub detail: Option<String>,
}

/// Login material: either `cookies` (recommended; must include BDUSS and
/// STOKEN entries) or `bduss` with an optional `stoken`.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct BaiduPcsLoginRequest {
    pub bduss: Option<String>,
    pub stoken: Option<String>,
    pub cookies: Option<String>,
    pub binary_path: Option<String>,
    pub config_dir: Option<String>,
    /// Store the material server-side (keyed by config dir) so upload jobs
    /// can re-login automatically when the session expires. Logout forgets
    /// it.
    pub remember: bool,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct BaiduPcsLoginResponse {
    pub success: bool,
    pub uid: Option<u64>,
    pub username: Option<String>,
    /// The `remember` flag was honored and the material is stored.
    pub credentials_stored: bool,
    /// Scrubbed CLI output tail for diagnostics.
    pub message: String,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct BaiduPcsLogoutResponse {
    pub success: bool,
    pub message: String,
}

/// Last [`MAX_DETAIL_CHARS`] characters of trimmed CLI output — the CLI
/// prints its verdict last, so the tail carries the diagnosis.
fn output_tail(text: &str) -> String {
    let trimmed = text.trim();
    let char_count = trimmed.chars().count();
    if char_count <= MAX_DETAIL_CHARS {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .skip(char_count - MAX_DETAIL_CHARS)
        .collect()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

#[utoipa::path(
    post,
    path = "/api/tools/baidupcs/status",
    tag = "tools",
    request_body = BaiduPcsToolRequest,
    responses(
        (status = 200, description = "Binary and login-session status", body = BaiduPcsStatusResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn baidupcs_status(
    Json(request): Json<BaiduPcsToolRequest>,
) -> ApiResult<Json<BaiduPcsStatusResponse>> {
    let binary_path = baidupcs::resolve_binary_path(request.binary_path.as_deref());
    let config_dir = non_empty(request.config_dir.as_deref());

    let has_stored_credentials = match baidupcs::authenticator() {
        Some(authenticator) => authenticator.has_credentials(config_dir).await,
        None => false,
    };
    let mut response = BaiduPcsStatusResponse {
        resolved_binary_path: binary_path.clone(),
        binary_ok: false,
        version: None,
        logged_in: false,
        uid: None,
        username: None,
        quota_used_bytes: None,
        quota_total_bytes: None,
        has_stored_credentials,
        detail: None,
    };

    // Read side of the session lock: safe next to a running upload, blocked
    // only while a login/logout briefly holds the write side.
    let _guard = baidupcs::cli_lock().read().await;

    let mut version_cmd = baidupcs::base_command(&binary_path, config_dir);
    version_cmd.arg("-v");
    match baidupcs::run_capture(version_cmd, None, STATUS_PROBE_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            response.binary_ok = true;
            response.version = output
                .stdout
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string);
        }
        Ok(output) => {
            response.detail = Some(output_tail(&output.combined()));
            return Ok(Json(response));
        }
        Err(e) => {
            response.detail = Some(output_tail(&e.to_string()));
            return Ok(Json(response));
        }
    }

    let mut who_cmd = baidupcs::base_command(&binary_path, config_dir);
    who_cmd.arg("who");
    match baidupcs::run_capture(who_cmd, None, STATUS_PROBE_TIMEOUT).await {
        Ok(output) => match baidupcs::parse_who(&output.stdout) {
            Some(account) if account.uid > 0 => {
                response.logged_in = true;
                response.uid = Some(account.uid);
                response.username = Some(account.username);
            }
            Some(_) => {}
            None => {
                response.detail = Some(output_tail(&output.combined()));
                return Ok(Json(response));
            }
        },
        Err(e) => {
            response.detail = Some(output_tail(&e.to_string()));
            return Ok(Json(response));
        }
    }

    if response.logged_in {
        let mut quota_cmd = baidupcs::base_command(&binary_path, config_dir);
        quota_cmd.arg("quota");
        if let Ok(output) = baidupcs::run_capture(quota_cmd, None, STATUS_PROBE_TIMEOUT).await {
            match baidupcs::parse_quota(&output.stdout) {
                Some(quota) => {
                    response.quota_used_bytes = Some(quota.used_bytes);
                    response.quota_total_bytes = Some(quota.total_bytes);
                }
                None => response.detail = Some(output_tail(&output.combined())),
            }
        }
    }

    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/tools/baidupcs/login",
    tag = "tools",
    request_body = BaiduPcsLoginRequest,
    responses(
        (status = 200, description = "Login attempt result (success flag inside)", body = BaiduPcsLoginResponse),
        (status = 400, description = "Neither cookies nor BDUSS provided", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "An upload or another login is using the BaiduPCS-Go session", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
#[tracing::instrument(skip_all)]
pub async fn baidupcs_login(
    Json(request): Json<BaiduPcsLoginRequest>,
) -> ApiResult<Json<BaiduPcsLoginResponse>> {
    let binary_path = baidupcs::resolve_binary_path(request.binary_path.as_deref());
    let config_dir = non_empty(request.config_dir.as_deref());

    // The credential values end up on the child argv, so they must never be
    // logged; this handler is skip_all-instrumented and `run_login` scrubs
    // any echo out of relayed output and errors.
    let material = baidupcs::LoginMaterial {
        cookies: non_empty(request.cookies.as_deref()).map(str::to_string),
        bduss: non_empty(request.bduss.as_deref()).map(str::to_string),
        stoken: non_empty(request.stoken.as_deref()).map(str::to_string),
    };
    if !material.is_usable() {
        return Err(ApiError::validation(
            "Provide either a cookie string or a BDUSS value",
        ));
    }

    // `login` rewrites the session store, so it must not run while uploads
    // hold the read side; reject instead of queueing behind a long upload.
    let Ok(_guard) = baidupcs::cli_lock().try_write() else {
        return Err(ApiError::conflict(
            "BaiduPCS-Go is busy (an upload or another login is in progress); try again later",
        ));
    };

    let outcome = baidupcs::run_login(&binary_path, config_dir, &material, baidupcs::LOGIN_TIMEOUT)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut message = output_tail(&outcome.message);

    let mut credentials_stored = false;
    if outcome.success && request.remember {
        match baidupcs::authenticator() {
            Some(authenticator) => {
                match authenticator.save_credentials(config_dir, &material).await {
                    Ok(()) => credentials_stored = true,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to store BaiduPCS-Go credentials");
                        message =
                            format!("{message}\n(Storing credentials for auto re-login failed)");
                    }
                }
            }
            None => {
                message = format!("{message}\n(Credential storage is unavailable)");
            }
        }
    }

    let mut response = BaiduPcsLoginResponse {
        success: outcome.success,
        uid: None,
        username: None,
        credentials_stored,
        message,
    };

    if outcome.success {
        let mut who_cmd = baidupcs::base_command(&binary_path, config_dir);
        who_cmd.arg("who");
        if let Ok(who_output) = baidupcs::run_capture(who_cmd, None, STATUS_PROBE_TIMEOUT).await
            && let Some(account) = baidupcs::parse_who(&who_output.stdout)
            && account.uid > 0
        {
            response.uid = Some(account.uid);
            response.username = Some(account.username);
        }
    }

    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/tools/baidupcs/logout",
    tag = "tools",
    request_body = BaiduPcsToolRequest,
    responses(
        (status = 200, description = "Logout attempt result (success flag inside)", body = BaiduPcsLogoutResponse),
        (status = 409, description = "An upload or a login is using the BaiduPCS-Go session", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn baidupcs_logout(
    Json(request): Json<BaiduPcsToolRequest>,
) -> ApiResult<Json<BaiduPcsLogoutResponse>> {
    let binary_path = baidupcs::resolve_binary_path(request.binary_path.as_deref());
    let config_dir = non_empty(request.config_dir.as_deref());

    let Ok(_guard) = baidupcs::cli_lock().try_write() else {
        return Err(ApiError::conflict(
            "BaiduPCS-Go is busy (an upload or a login is in progress); try again later",
        ));
    };

    // Logging out also means "stop re-logging in automatically", so the
    // stored material is forgotten even if the CLI logout itself fails
    // (e.g. missing binary).
    let mut store_note = None;
    if let Some(authenticator) = baidupcs::authenticator()
        && let Err(e) = authenticator.delete_credentials(config_dir).await
    {
        tracing::warn!(error = %e, "Failed to delete stored BaiduPCS-Go credentials");
        store_note = Some("(Forgetting stored credentials failed)");
    }

    let mut cmd = baidupcs::base_command(&binary_path, config_dir);
    cmd.arg("logout");
    // `logout` asks `确认退出百度帐号: <name> ? (y/n) >` on stdin.
    let output = baidupcs::run_capture(cmd, Some(b"y\n"), LOGOUT_TIMEOUT)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let combined = output.combined();
    let mut message = output_tail(&combined);
    if let Some(note) = store_note {
        message = format!("{message}\n{note}");
    }
    Ok(Json(BaiduPcsLogoutResponse {
        success: combined.contains(baidupcs::LOGOUT_SUCCESS_MARKER),
        message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_tail_keeps_the_end_of_long_output() {
        let long = "x".repeat(MAX_DETAIL_CHARS) + "TAIL";
        let tail = output_tail(&long);
        assert_eq!(tail.chars().count(), MAX_DETAIL_CHARS);
        assert!(tail.ends_with("TAIL"));
        assert_eq!(output_tail("  short  "), "short");
    }

    #[test]
    fn non_empty_filters_blank_overrides() {
        assert_eq!(non_empty(Some("  /cfg ")), Some("/cfg"));
        assert_eq!(non_empty(Some("   ")), None);
        assert_eq!(non_empty(None), None);
    }
}
