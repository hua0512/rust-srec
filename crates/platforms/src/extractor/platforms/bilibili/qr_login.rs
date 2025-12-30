//! Bilibili QR code login utilities using TV API.
//!
//! This module provides functions for QR code generation and polling
//! for Bilibili login using the TV endpoint, which directly provides
//! cookies and refresh_token.

use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

use crate::extractor::default::DEFAULT_UA;

/// AppKey for TV login (云视听小电视).
const TV_APPKEY: &str = "4409e2ce8ffd12b8";
const TV_APPSEC: &str = "59b43e04ad6965f34319062b478f83dd";

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
}

/// Sign parameters using MD5 for TV API.
fn sign_params(params: &mut Vec<(&str, String)>) -> String {
    params.sort_by(|a, b| a.0.cmp(b.0));
    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");
    let to_sign = format!("{}{}", query, TV_APPSEC);
    let mut hasher = Md5::new();
    hasher.update(to_sign.as_bytes());
    format!("{:x}", hasher.finalize())
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
    let sign = sign_params(&mut params);
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
    let sign = sign_params(&mut params);
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
            });
        }
        86090 => {
            return Ok(QrPollResult {
                status: QrPollStatus::ScannedNotConfirmed,
                message,
                cookies: None,
                refresh_token: None,
            });
        }
        86039 => {
            return Ok(QrPollResult {
                status: QrPollStatus::NotScanned,
                message,
                cookies: None,
                refresh_token: None,
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
            });
        }
    };

    let refresh_token = data
        .get("refresh_token")
        .and_then(|t| t.as_str())
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

    if cookies.is_some() || refresh_token.is_some() {
        Ok(QrPollResult {
            status: QrPollStatus::Success,
            message,
            cookies,
            refresh_token,
        })
    } else {
        Ok(QrPollResult {
            status: QrPollStatus::NotScanned,
            message,
            cookies: None,
            refresh_token: None,
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
        let sign = sign_params(&mut params);
        // Just verify it produces a 32-char hex string
        assert_eq!(sign.len(), 32);
        assert!(sign.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
