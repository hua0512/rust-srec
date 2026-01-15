//! Bilibili cookie refresh utilities.
//!
//! This module provides the core logic for Bilibili cookie refresh:
//! 1. Check cookie status via /cookie/info API
//! 2. Generate CorrespondPath using RSA-OAEP encryption
//! 3. Fetch refresh_csrf from HTML page
//! 4. Perform refresh and confirm

use reqwest::Client;
use rsa::pkcs8::DecodePublicKey;
use rsa::rand_core::OsRng;
use rsa::{Oaep, RsaPublicKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use tracing::debug;

use super::cookie_utils::{
    extract_cookie_value, extract_refresh_csrf, parse_set_cookies, rebuild_cookies,
    strip_refresh_token, urls,
};

/// Bilibili's RSA public key for CorrespondPath generation.
const BILIBILI_PUBKEY_PEM: &str = r#"-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDLgd2OAkcGVtoE3ThUREbio0Eg
Uc/prcajMKXvkCKFCWhJYJcLkcM2DKKcSeFpD/j6Boy538YXnR6VhcuUJOhH2x71
nzPjfdTcqMz7djHum0qSZA0AyCBDABUqCrfNgCiJ00Ra7GmRj+YCK1NJEuewlb40
JNrRuoEUXpabUzGB8QIDAQAB
-----END PUBLIC KEY-----
"#;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

#[derive(Debug, Error)]
pub enum CookieRefreshError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Missing cookie: {0}")]
    MissingCookie(&'static str),
    #[error("Missing refresh token")]
    MissingRefreshToken,
    #[error("Refresh failed: {0}")]
    RefreshFailed(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Cookie status check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CookieStatus {
    /// Cookies are valid, no refresh needed
    Valid,
    /// Cookies need refresh before deadline
    NeedsRefresh { deadline_timestamp: Option<u64> },
    /// Cookies are invalid (not logged in)
    Invalid { reason: String, code: Option<i64> },
}

/// Refreshed credentials result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshedCookies {
    /// New cookie string
    pub cookies: String,
    /// New refresh token
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    code: i64,
    data: Option<UserInfoData>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserInfoData {
    uid: u64,
}

async fn try_fetch_uid(client: &Client, cookies: &str) -> Result<Option<u64>, CookieRefreshError> {
    let response = client
        .get(urls::USER_INFO)
        .header("Cookie", cookies)
        .header("User-Agent", USER_AGENT)
        .header(reqwest::header::REFERER, "https://live.bilibili.com")
        .send()
        .await?;

    let body: UserInfoResponse = response
        .json()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    if body.code != 0 {
        let message = body.message.unwrap_or_else(|| "Unknown error".to_string());
        return Err(CookieRefreshError::Parse(format!(
            "get_user_info returned error {}: {}",
            body.code, message
        )));
    }

    Ok(body.data.map(|d| d.uid))
}

async fn persist_uid_cookie_best_effort(client: &Client, cookies: &str) -> String {
    if extract_cookie_value(cookies, "DedeUserID").is_some() {
        return cookies.to_string();
    }

    let uid = match try_fetch_uid(client, cookies).await {
        Ok(Some(uid)) => uid,
        _ => return cookies.to_string(),
    };
    debug!("Fetched UID: {}", uid);

    let mut updates = std::collections::HashMap::new();
    updates.insert("DedeUserID".to_string(), uid.to_string());
    rebuild_cookies(cookies, &updates)
}

/// Load the Bilibili RSA public key.
fn load_public_key() -> Result<RsaPublicKey, CookieRefreshError> {
    RsaPublicKey::from_public_key_pem(BILIBILI_PUBKEY_PEM)
        .map_err(|e| CookieRefreshError::Crypto(e.to_string()))
}

/// Generate CorrespondPath using RSA-OAEP encryption.
pub fn generate_correspond_path(timestamp_ms: u64) -> Result<String, CookieRefreshError> {
    let public_key = load_public_key()?;
    let message = format!("refresh_{}", timestamp_ms);
    let mut rng = OsRng;

    let padding = Oaep::new::<Sha256>();
    let encrypted = public_key
        .encrypt(&mut rng, padding, message.as_bytes())
        .map_err(|e| CookieRefreshError::Crypto(e.to_string()))?;

    Ok(hex::encode(&encrypted))
}

/// Check if cookies need refresh.
pub async fn check_cookie_status(
    client: &Client,
    cookies: &str,
) -> Result<CookieStatus, CookieRefreshError> {
    let cookies = strip_refresh_token(cookies);
    let bili_jct = extract_cookie_value(&cookies, "bili_jct")
        .ok_or(CookieRefreshError::MissingCookie("bili_jct"))?;

    let url = format!("{}?csrf={}", urls::COOKIE_INFO, bili_jct);

    let response = client
        .get(&url)
        .header("Cookie", &cookies)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);

    match code {
        0 => {
            let needs_refresh = body
                .get("data")
                .and_then(|d| d.get("refresh"))
                .and_then(|r| r.as_bool())
                .unwrap_or(false);

            if needs_refresh {
                let timestamp = body
                    .get("data")
                    .and_then(|d| d.get("timestamp"))
                    .and_then(|t| t.as_u64());
                Ok(CookieStatus::NeedsRefresh {
                    deadline_timestamp: timestamp,
                })
            } else {
                Ok(CookieStatus::Valid)
            }
        }
        -101 => Ok(CookieStatus::Invalid {
            reason: "Not logged in".to_string(),
            code: Some(-101),
        }),
        _ => {
            let message = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            Ok(CookieStatus::Invalid {
                reason: format!("Error {}: {}", code, message),
                code: Some(code),
            })
        }
    }
}

/// Perform cookie refresh.
///
/// Returns the new cookies and refresh_token on success.
pub async fn refresh_cookies(
    client: &Client,
    cookies: &str,
    refresh_token: &str,
) -> Result<RefreshedCookies, CookieRefreshError> {
    let cookies = strip_refresh_token(cookies);
    let bili_jct = extract_cookie_value(&cookies, "bili_jct")
        .ok_or(CookieRefreshError::MissingCookie("bili_jct"))?;

    // Step 1: Generate CorrespondPath using current timestamp
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CookieRefreshError::Internal("System time error".to_string()))?
        .as_millis() as u64;

    let correspond_path = generate_correspond_path(timestamp_ms)?;

    // Step 2: Get refresh_csrf from HTML page
    let correspond_url = format!("{}{}", urls::CORRESPOND, correspond_path);

    let correspond_response = client
        .get(&correspond_url)
        .header("Cookie", &cookies)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;

    let status = correspond_response.status();
    let html = correspond_response
        .text()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    let refresh_csrf = extract_refresh_csrf(&html).ok_or_else(|| {
        // Log more details to help diagnose the issue
        let preview = if html.len() > 500 {
            format!("{}...(truncated)", &html[..500])
        } else {
            html.clone()
        };
        CookieRefreshError::Parse(format!(
            "Failed to extract refresh_csrf (HTTP status: {}, response preview: {})",
            status, preview
        ))
    })?;

    // Step 3: Perform refresh
    let refresh_params = [
        ("csrf", bili_jct.as_str()),
        ("refresh_csrf", refresh_csrf.as_str()),
        ("source", "main_web"),
        ("refresh_token", refresh_token),
    ];

    let refresh_response = client
        .post(urls::REFRESH)
        .header("Cookie", &cookies)
        .header("User-Agent", USER_AGENT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&refresh_params)
        .send()
        .await?;

    // Extract new cookies from Set-Cookie headers
    let new_cookies_map = parse_set_cookies(refresh_response.headers());

    // Parse response body for API result
    let refresh_body: serde_json::Value = refresh_response
        .json()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    // Check API response code first
    let code = refresh_body
        .get("code")
        .and_then(|c| c.as_i64())
        .unwrap_or(-1);
    if code != 0 {
        let message = refresh_body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error");
        return Err(CookieRefreshError::RefreshFailed(format!(
            "Refresh API error {}: {}",
            code, message
        )));
    }

    let new_bili_jct = new_cookies_map.get("bili_jct").ok_or_else(|| {
        CookieRefreshError::RefreshFailed(format!(
            "Missing bili_jct in response (received cookies: {:?})",
            new_cookies_map.keys().collect::<Vec<_>>()
        ))
    })?;

    let new_refresh_token = refresh_body
        .get("data")
        .and_then(|d| d.get("refresh_token"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            CookieRefreshError::RefreshFailed("Missing new refresh_token in response".to_string())
        })?;

    // Step 4: Confirm refresh (invalidate old token)
    // Use a full cookie header to maximize compatibility (some flows require more than just SESSDATA/bili_jct).
    let provisional_cookies = rebuild_cookies(&cookies, &new_cookies_map);

    let confirm_params = [
        ("csrf", new_bili_jct.as_str()),
        ("refresh_token", refresh_token),
    ];

    let confirm_response = client
        .post(urls::CONFIRM)
        .header("Cookie", &provisional_cookies)
        .header("User-Agent", USER_AGENT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&confirm_params)
        .send()
        .await?;

    // Check confirm response (non-zero is usually ok)
    let _confirm_body: serde_json::Value = confirm_response
        .json()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    // Build full cookie string preserving other cookies
    let final_cookies = rebuild_cookies(&cookies, &new_cookies_map);
    let final_cookies = persist_uid_cookie_best_effort(client, &final_cookies).await;

    Ok(RefreshedCookies {
        cookies: final_cookies,
        refresh_token: new_refresh_token.to_string(),
    })
}

/// Validate cookies by making an authenticated API call.
pub async fn validate_cookies(client: &Client, cookies: &str) -> Result<bool, CookieRefreshError> {
    let cookies = strip_refresh_token(cookies);
    let response = client
        .get(urls::NAV)
        .header("Cookie", &cookies)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| CookieRefreshError::Parse(e.to_string()))?;

    // code 0 = logged in, -101 = not logged in
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    Ok(code == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_correspond_path() {
        let result = generate_correspond_path(1234567890123);
        assert!(result.is_ok());
        let path = result.unwrap();
        // Should be a hex string
        assert!(!path.is_empty());
        assert!(path.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
