//! Shared TikTok helpers: `ttwid` session cookie acquisition and the common
//! web query parameters sent with webcast requests.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;
use tracing::debug;

use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;

/// ByteDance unified `ttwid` issuer shared with Douyin; TikTok web uses
/// `aid` 1768 for registration (the webcast `aid` 1988 is rejected).
const UNION_REGISTER_URL: &str = "https://ttwid.bytedance.com/ttwid/union/register/";

pub(crate) const TIKTOK_WEB_URL: &str = "https://www.tiktok.com";
pub(crate) const WEBCAST_AID: &str = "1988";
pub(crate) const WEBCAST_APP_NAME: &str = "tiktok_web";

/// A `ttwid` is valid far longer than this, but refreshing keeps a burned
/// session (rate limited by TikTok's WAF) from sticking around forever.
const TTWID_CACHE_EXPIRATION: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone)]
struct CachedTtwid {
    value: String,
    fetched_at: Instant,
}

static GLOBAL_TTWID: LazyLock<Arc<Mutex<Option<CachedTtwid>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));
static GLOBAL_TTWID_FETCH_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

/// Browser-ish query parameters common to the `api-live` and `webcast`
/// endpoints. Values are static: the endpoints used by the extractor and the
/// danmu provider do not validate them against the session.
pub(crate) fn common_query_params() -> Vec<(&'static str, String)> {
    let browser_version = DEFAULT_UA
        .strip_prefix("Mozilla/")
        .unwrap_or(DEFAULT_UA)
        .to_string();
    vec![
        ("aid", WEBCAST_AID.to_string()),
        ("app_name", WEBCAST_APP_NAME.to_string()),
        ("app_language", "en".to_string()),
        ("webcast_language", "en".to_string()),
        ("device_platform", "web_pc".to_string()),
        ("channel", WEBCAST_APP_NAME.to_string()),
        ("browser_language", "en-US".to_string()),
        ("browser_name", "Mozilla".to_string()),
        ("browser_online", "true".to_string()),
        ("browser_platform", "Win32".to_string()),
        ("browser_version", browser_version),
        ("cookie_enabled", "true".to_string()),
        ("focus_state", "true".to_string()),
        ("is_fullscreen", "false".to_string()),
        ("is_page_visible", "true".to_string()),
        ("os", "windows".to_string()),
        ("screen_height", "1080".to_string()),
        ("screen_width", "1920".to_string()),
        ("tz_name", "UTC".to_string()),
        ("user_is_login", "false".to_string()),
    ]
}

fn cached_global_ttwid() -> Option<String> {
    let guard = GLOBAL_TTWID.lock().ok()?;
    guard
        .as_ref()
        .filter(|cached| cached.fetched_at.elapsed() < TTWID_CACHE_EXPIRATION)
        .map(|cached| cached.value.clone())
}

/// Registers a fresh `ttwid` with the ByteDance union endpoint.
///
/// # Errors
///
/// Returns an [`ExtractorError`] if the request fails, returns a non-success
/// status, or the response lacks a `ttwid` cookie.
pub(crate) async fn fetch_ttwid(client: &Client) -> Result<String, ExtractorError> {
    let body = json!({
        "region": "va",
        "aid": 1768,
        "needFid": false,
        "service": TIKTOK_WEB_URL,
        "migrate_priority": 0,
        "cbUrlProtocol": "https",
        "misleep": 100
    });

    let response = client
        .post(UNION_REGISTER_URL)
        .header(reqwest::header::USER_AGENT, DEFAULT_UA)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| {
            cookie
                .split(';')
                .next()?
                .trim()
                .strip_prefix("ttwid=")
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            ExtractorError::ValidationError(
                "TikTok ttwid registration response did not set a ttwid cookie".to_string(),
            )
        })
}

/// Returns a process-wide `ttwid`, registering one when the cache is empty
/// or stale. Concurrent callers share a single registration request.
///
/// # Errors
///
/// Propagates [`fetch_ttwid`] failures; nothing is cached on failure so the
/// next caller retries.
pub(crate) async fn ensure_global_ttwid(client: &Client) -> Result<String, ExtractorError> {
    if let Some(existing) = cached_global_ttwid() {
        return Ok(existing);
    }

    let _fetch_guard = GLOBAL_TTWID_FETCH_LOCK.lock().await;
    if let Some(existing) = cached_global_ttwid() {
        return Ok(existing);
    }

    debug!("Registering a fresh TikTok ttwid");
    let ttwid = fetch_ttwid(client).await?;
    if let Ok(mut guard) = GLOBAL_TTWID.lock() {
        *guard = Some(CachedTtwid {
            value: ttwid.clone(),
            fetched_at: Instant::now(),
        });
    }
    Ok(ttwid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_params_carry_webcast_identity() {
        let params = common_query_params();
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("aid"), Some("1988"));
        assert_eq!(get("app_name"), Some("tiktok_web"));
        assert!(get("browser_version").is_some_and(|v| v.contains("Chrome/")));
        assert!(get("browser_version").is_none_or(|v| !v.starts_with("Mozilla/")));
    }
}
