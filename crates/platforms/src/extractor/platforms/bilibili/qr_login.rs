//! Bilibili QR code login utilities using TV API.
//!
//! This module provides functions for QR code generation and polling
//! for Bilibili login using the TV endpoint, which directly provides
//! cookies and refresh_token.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

use super::cookie_utils::{extract_cookie_value, rebuild_cookies, urls};
use super::utils::{TV_APPKEY, TV_APPSEC, sign_params};
use crate::extractor::default::DEFAULT_UA;

const TV_QR_GENERATE_URL: &str =
    "https://passport.bilibili.com/x/passport-tv-login/qrcode/auth_code";
const TV_QR_POLL_URL: &str = "https://passport.bilibili.com/x/passport-tv-login/qrcode/poll";

#[derive(Debug, Error)]
pub enum QrLoginError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("API error: {0}")]
    Api(String),
    #[error("System time error")]
    SystemTime,
}

/// QR code generation response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrGenerateResponse {
    /// URL to encode as QR code
    pub url: String,
    /// Auth code for polling
    pub auth_code: String,
}

/// QR code poll status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QrPollStatus {
    /// QR not yet scanned
    NotScanned,
    /// QR scanned but not confirmed
    ScannedNotConfirmed,
    /// QR code expired
    Expired,
    /// Login successful
    Success,
}

/// QR code poll result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrPollResult {
    /// Poll status
    pub status: QrPollStatus,
    /// Response message from API
    pub message: String,
    /// Cookies (if success)
    pub cookies: Option<String>,
    /// Refresh token (if success)
    pub refresh_token: Option<String>,
    /// OAuth2 access token (if success, from token_info)
    pub access_token: Option<String>,
}

/// Generate a QR code for Bilibili TV login.
///
/// Returns the URL to encode as QR code and the auth_code for polling.
pub async fn generate_qr(client: &Client) -> Result<QrGenerateResponse, QrLoginError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| QrLoginError::SystemTime)?
        .as_secs()
        .to_string();

    let mut params = vec![
        ("appkey", TV_APPKEY.to_string()),
        ("local_id", "0".to_string()),
        ("ts", ts),
    ];
    let sign = sign_params(&mut params, TV_APPSEC);
    params.push(("sign", sign));

    let response = client
        .post(TV_QR_GENERATE_URL)
        .header("User-Agent", DEFAULT_UA)
        .form(&params)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| QrLoginError::Parse(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error");
        return Err(QrLoginError::Api(format!("QR generate failed: {}", msg)));
    }

    let data = body
        .get("data")
        .ok_or_else(|| QrLoginError::Parse("No data field".to_string()))?;
    let url = data
        .get("url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| QrLoginError::Parse("No url field".to_string()))?;
    let auth_code = data
        .get("auth_code")
        .and_then(|c| c.as_str())
        .ok_or_else(|| QrLoginError::Parse("No auth_code field".to_string()))?;

    Ok(QrGenerateResponse {
        url: url.to_string(),
        auth_code: auth_code.to_string(),
    })
}

/// Poll the status of a QR code login.
///
/// Returns the poll status and, on success, the cookies and refresh_token.
pub async fn poll_qr(client: &Client, auth_code: &str) -> Result<QrPollResult, QrLoginError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| QrLoginError::SystemTime)?
        .as_secs()
        .to_string();

    let mut params = vec![
        ("appkey", TV_APPKEY.to_string()),
        ("auth_code", auth_code.to_string()),
        ("local_id", "0".to_string()),
        ("ts", ts),
    ];
    let sign = sign_params(&mut params, TV_APPSEC);
    params.push(("sign", sign));

    let response = client
        .post(TV_QR_POLL_URL)
        .header("User-Agent", DEFAULT_UA)
        .form(&params)
        .send()
        .await?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| QrLoginError::Parse(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    let message = body
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    tracing::debug!(code, message = %message, "Bilibili QR poll response");

    match code {
        0 => {} // Success, continue parsing
        86038 => {
            return Ok(QrPollResult {
                status: QrPollStatus::Expired,
                message,
                cookies: None,
                refresh_token: None,
                access_token: None,
            });
        }
        86090 => {
            return Ok(QrPollResult {
                status: QrPollStatus::ScannedNotConfirmed,
                message,
                cookies: None,
                refresh_token: None,
                access_token: None,
            });
        }
        86039 => {
            return Ok(QrPollResult {
                status: QrPollStatus::NotScanned,
                message,
                cookies: None,
                refresh_token: None,
                access_token: None,
            });
        }
        _ => {
            return Err(QrLoginError::Api(format!(
                "Poll failed: {} ({})",
                message, code
            )));
        }
    }

    let data = match body.get("data") {
        Some(d) if !d.is_null() => d,
        _ => {
            return Ok(QrPollResult {
                status: QrPollStatus::NotScanned,
                message,
                cookies: None,
                refresh_token: None,
                access_token: None,
            });
        }
    };

    // Extract token_info fields (access_token, mid)
    let token_info = data.get("token_info");
    let access_token = token_info
        .and_then(|ti| ti.get("access_token"))
        .and_then(|t| t.as_str())
        .map(String::from);
    let token_mid = token_info
        .and_then(|ti| ti.get("mid"))
        .and_then(|m| m.as_u64());

    // Prefer refresh_token from token_info, fall back to top-level
    let refresh_token = token_info
        .and_then(|ti| ti.get("refresh_token"))
        .and_then(|t| t.as_str())
        .or_else(|| data.get("refresh_token").and_then(|t| t.as_str()))
        .map(String::from);

    let cookies = data.get("cookie_info").and_then(|ci| {
        ci.get("cookies")
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
    });

    // Ensure DedeUserID is present in cookies — use mid from token_info first,
    // then fall back to a live API call.
    let cookies = if let Some(cookies) = cookies {
        if extract_cookie_value(&cookies, "DedeUserID").is_some() {
            Some(cookies)
        } else if let Some(mid) = token_mid {
            // Use mid from token_info directly — no extra API call needed.
            let mut updates = std::collections::HashMap::new();
            updates.insert("DedeUserID".to_string(), mid.to_string());
            Some(rebuild_cookies(&cookies, &updates))
        } else {
            // Fallback: fetch uid from live API
            let maybe_uid = match client
                .get(urls::USER_INFO)
                .header("Cookie", &cookies)
                .header("User-Agent", DEFAULT_UA)
                .header(reqwest::header::REFERER, "https://live.bilibili.com")
                .send()
                .await
            {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(v) => v
                        .get("data")
                        .and_then(|d| d.get("uid"))
                        .and_then(|u| u.as_u64()),
                    Err(_) => None,
                },
                Err(_) => None,
            };

            if let Some(uid) = maybe_uid {
                let mut updates = std::collections::HashMap::new();
                updates.insert("DedeUserID".to_string(), uid.to_string());
                Some(rebuild_cookies(&cookies, &updates))
            } else {
                Some(cookies)
            }
        }
    } else {
        None
    };

    if cookies.is_some() && refresh_token.is_some() {
        Ok(QrPollResult {
            status: QrPollStatus::Success,
            message,
            cookies,
            refresh_token,
            access_token,
        })
    } else {
        Ok(QrPollResult {
            status: QrPollStatus::NotScanned,
            message,
            cookies: None,
            refresh_token: None,
            access_token: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_params() {
        let mut params = vec![
            ("appkey", TV_APPKEY.to_string()),
            ("local_id", "0".to_string()),
            ("ts", "1234567890".to_string()),
        ];
        let sign = sign_params(&mut params, TV_APPSEC);
        // Just verify it produces a 32-char hex string
        assert_eq!(sign.len(), 32);
        assert!(sign.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
