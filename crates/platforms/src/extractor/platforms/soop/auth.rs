//! SOOP session validation and password login helpers.
//!
//! Used by the extractor (reactive login) and by the app credential manager
//! (proactive check + re-login with persisted cookies).

use reqwest::Client;
use reqwest::header::SET_COOKIE;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::platforms::soop::models::SoopLoginResponse;

const LOGIN_URL: &str = "https://login.sooplive.com/app/LoginAction.php";
const AUTH_CHECK_URL: &str = "https://afevent2.sooplive.com/api/get_private_info.php";
const ORIGIN: &str = "https://play.sooplive.com";

#[derive(Debug, Deserialize)]
struct PrivateInfoResponse {
    #[serde(rename = "CHANNEL")]
    channel: Option<PrivateInfoChannel>,
}

#[derive(Debug, Deserialize)]
struct PrivateInfoChannel {
    #[serde(rename = "LOGIN_ID", default)]
    login_id: Option<String>,
}

/// Returns true when the cookie string represents a logged-in SOOP session.
pub async fn validate_session(client: &Client, cookies: &str) -> Result<bool, ExtractorError> {
    let cookies = cookies.trim();
    if cookies.is_empty() {
        return Ok(false);
    }

    let response = client
        .get(AUTH_CHECK_URL)
        .header(reqwest::header::USER_AGENT, DEFAULT_UA)
        .header(reqwest::header::ORIGIN, ORIGIN)
        .header(reqwest::header::REFERER, ORIGIN)
        .header(reqwest::header::COOKIE, cookies)
        .send()
        .await?
        .error_for_status()?;

    let body: PrivateInfoResponse = response.json().await.map_err(|e| {
        ExtractorError::ValidationError(format!("SOOP private info parse error: {e}"))
    })?;

    let login_id = body
        .channel
        .and_then(|c| c.login_id)
        .filter(|s| !s.trim().is_empty());

    let ok = login_id.is_some();
    debug!(valid = ok, "SOOP session validation");
    Ok(ok)
}

/// Log in with username/password and return a Cookie header string (`k=v; …`).
pub async fn login_for_cookies(
    client: &Client,
    username: &str,
    password: &str,
) -> Result<String, ExtractorError> {
    if username.is_empty() || password.is_empty() {
        return Err(ExtractorError::ValidationError(
            "SOOP username/password is not configured".to_string(),
        ));
    }

    if username.len() < 6 || password.len() < 10 {
        warn!(
            username_len = username.len(),
            password_len = password.len(),
            "SOOP credentials are shorter than expected"
        );
    }

    let form = [
        ("szWork", "login"),
        ("szType", "json"),
        ("szUid", username),
        ("szPassword", password),
        ("isSaveId", "true"),
        ("isSavePw", "false"),
        ("isSaveJoin", "false"),
        ("isLoginRetain", "Y"),
    ];

    let response = client
        .post(LOGIN_URL)
        .header(reqwest::header::USER_AGENT, DEFAULT_UA)
        .header(reqwest::header::ORIGIN, ORIGIN)
        .header(reqwest::header::REFERER, ORIGIN)
        .form(&form)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(ExtractorError::ValidationError(format!(
            "SOOP login returned HTTP {}",
            response.status()
        )));
    }

    let cookie_header = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("; ");

    let login_response = response.json::<SoopLoginResponse>().await?;
    if login_response.result != 1 {
        return Err(ExtractorError::ValidationError(
            "SOOP login failed".to_string(),
        ));
    }

    if cookie_header.is_empty() {
        return Err(ExtractorError::ValidationError(
            "SOOP login succeeded without session cookies".to_string(),
        ));
    }

    debug!("SOOP login produced session cookies");
    Ok(cookie_header)
}
