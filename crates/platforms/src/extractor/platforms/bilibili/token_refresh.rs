//! OAuth2/APP token refresh for Bilibili.
//!
//! This module implements the biliup-style token refresh flow:
//! 1. **Validate** via `/x/passport-login/oauth2/info` (Android keypair)
//! 2. **Refresh** via `/x/passport-login/oauth2/refresh_token` (BiliTV keypair — must match login platform)
//!
//! This replaces the web cookie refresh flow which relied on RSA-OAEP encryption
//! and HTML page parsing.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

use super::utils::{ANDROID_APPKEY, ANDROID_APPSEC, TV_APPKEY, TV_APPSEC, sign_params};
use crate::extractor::default::DEFAULT_UA;

const OAUTH2_INFO_URL: &str = "https://passport.bilibili.com/x/passport-login/oauth2/info";
const OAUTH2_REFRESH_URL: &str =
    "https://passport.bilibili.com/x/passport-login/oauth2/refresh_token";

#[derive(Debug, Error)]
pub enum TokenRefreshError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("API error (code={code}): {message}")]
    Api { code: i64, message: String },
    #[error("System time error")]
    SystemTime,
}

/// Result of a successful token refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshedTokens {
    /// New cookie string (semicolon-separated key=value pairs from cookie_info)
    pub cookies: String,
    /// New OAuth2 access token
    pub access_token: String,
    /// New OAuth2 refresh token
    pub refresh_token: String,
    /// User mid
    pub mid: u64,
    /// Token validity in seconds (typically 2592000 = 30 days)
    pub expires_in: u64,
}

/// Validate an OAuth2 access token.
///
/// Uses the **Android** keypair for signing (mirrors biliup behavior).
///
/// Returns:
/// - `Ok(true)` if the token needs refresh (`data.refresh == true`)
/// - `Ok(false)` if the token is still valid (`data.refresh == false`)
pub async fn validate_token(
    client: &Client,
    access_token: &str,
) -> Result<bool, TokenRefreshError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TokenRefreshError::SystemTime)?
        .as_secs()
        .to_string();

    let mut params = vec![
        ("access_key", access_token.to_string()),
        ("actionKey", "appkey".to_string()),
        ("appkey", ANDROID_APPKEY.to_string()),
        ("ts", ts),
    ];
    let sign = sign_params(&mut params, ANDROID_APPSEC);
    params.push(("sign", sign));

    let response = client
        .get(OAUTH2_INFO_URL)
        .header("User-Agent", DEFAULT_UA)
        .query(&params)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| TokenRefreshError::Parse(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        return Err(TokenRefreshError::Api { code, message });
    }

    let data = body
        .get("data")
        .ok_or_else(|| TokenRefreshError::Parse("No data field".to_string()))?;

    let needs_refresh = data
        .get("refresh")
        .and_then(|r| r.as_bool())
        .unwrap_or(false);

    tracing::debug!(
        needs_refresh,
        mid = ?data.get("mid").and_then(|m| m.as_u64()),
        expires_in = ?data.get("expires_in").and_then(|e| e.as_u64()),
        "Bilibili OAuth2 token validation"
    );

    Ok(needs_refresh)
}

/// Refresh OAuth2 tokens.
///
/// Uses the **BiliTV** keypair for signing (must match the platform used for QR login).
///
/// Returns new cookies, access_token, refresh_token, mid, and expires_in.
pub async fn refresh_token(
    client: &Client,
    access_token: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, TokenRefreshError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TokenRefreshError::SystemTime)?
        .as_secs()
        .to_string();

    let mut params = vec![
        ("access_key", access_token.to_string()),
        ("actionKey", "appkey".to_string()),
        ("appkey", TV_APPKEY.to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("ts", ts),
    ];
    let sign = sign_params(&mut params, TV_APPSEC);
    params.push(("sign", sign));

    let response = client
        .post(OAUTH2_REFRESH_URL)
        .header("User-Agent", DEFAULT_UA)
        .form(&params)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| TokenRefreshError::Parse(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        return Err(TokenRefreshError::Api { code, message });
    }

    let data = body
        .get("data")
        .ok_or_else(|| TokenRefreshError::Parse("No data field".to_string()))?;

    // Extract token_info
    let token_info = data
        .get("token_info")
        .ok_or_else(|| TokenRefreshError::Parse("No token_info field".to_string()))?;

    let new_access_token = token_info
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| TokenRefreshError::Parse("No access_token in token_info".to_string()))?
        .to_string();

    let new_refresh_token = token_info
        .get("refresh_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| TokenRefreshError::Parse("No refresh_token in token_info".to_string()))?
        .to_string();

    let mid = token_info.get("mid").and_then(|m| m.as_u64()).unwrap_or(0);

    let expires_in = token_info
        .get("expires_in")
        .and_then(|e| e.as_u64())
        .unwrap_or(2592000);

    // Extract cookies from cookie_info
    let cookies = data
        .get("cookie_info")
        .and_then(|ci| ci.get("cookies"))
        .and_then(|arr| arr.as_array())
        .map(|cookies| {
            cookies
                .iter()
                .filter_map(|c| {
                    let name = c.get("name")?.as_str()?;
                    let value = c.get("value")?.as_str()?;
                    Some(format!("{}={}", name, value))
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();

    if cookies.is_empty() {
        return Err(TokenRefreshError::Parse(
            "No cookies in refresh response".to_string(),
        ));
    }

    // Ensure DedeUserID is present
    let cookies = if !cookies.contains("DedeUserID=") && mid > 0 {
        let mut updates = std::collections::HashMap::new();
        updates.insert("DedeUserID".to_string(), mid.to_string());
        super::cookie_utils::rebuild_cookies(&cookies, &updates)
    } else {
        cookies
    };

    tracing::debug!(mid, expires_in, "Bilibili OAuth2 token refresh successful");

    Ok(RefreshedTokens {
        cookies,
        access_token: new_access_token,
        refresh_token: new_refresh_token,
        mid,
        expires_in,
    })
}
