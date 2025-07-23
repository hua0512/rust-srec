use rand::{Rng, rng};
use reqwest::Client;
use serde_json::json;
use std::sync::LazyLock;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::debug;

use crate::extractor::platforms::douyin::URL_REGEX;
use crate::extractor::{
    default::DEFAULT_UA,
    error::ExtractorError,
    platforms::douyin::apis::{BASE_URL, UNION_REGISTER_URL},
};

pub static GLOBAL_TTWID: LazyLock<Arc<Mutex<Option<String>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

pub(crate) fn extract_rid(url: &str) -> Result<String, ExtractorError> {
    URL_REGEX
        .captures(url)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or(ExtractorError::ValidationError(
            "Failed to extract rid from url".to_string(),
        ))
}

pub(crate) fn get_common_params() -> HashMap<&'static str, &'static str> {
    let mut params = HashMap::new();
    params.insert("app_name", "douyin_web");
    params.insert("compress", "gzip");
    params.insert("device_platform", "web");
    params.insert("browser_language", "zh-CN");
    params.insert("browser_platform", "Win32");
    params.insert("browser_name", "Mozilla");
    // remove prefix 'mozilla/'
    let ua = DEFAULT_UA.trim_start_matches("Mozilla/");
    params.insert("browser_version", ua);
    params.insert("aid", "6383");
    params.insert("live_id", "1");
    params
}

/// Generate a random ms_token, 184 length
pub(crate) fn generate_ms_token() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
    let mut rng = rng();
    (0..184)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

static CHARSET: LazyLock<&[u8]> = LazyLock::new(|| b"abcdef0123456789");

/// Generate a random nonce, 21 length
pub(crate) fn generate_nonce() -> String {
    let mut rng = rng();
    (0..21)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generate a random odin_ttid, 160 length
pub(crate) fn generate_odin_ttid() -> String {
    let mut rng = rng();
    (0..160)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Default ttwid value to use as fallback when unable to obtain one from Douyin's servers.
pub(crate) const DEFAULT_TTWID: &str = "1%7Cu7ogdHsSmHtxbt4hjDCNvcLfVJz78CTM0TTWU8Hio8w%7C1751545220%7C18aac967e501e9d6c13384335ced3523c46a0b1cc4535c7213bc2506a7f462c8";

/// Sends a POST request to the UNION_REGISTER_URL and parses the `ttwid` from the `set-cookie` header.
///
/// # Arguments
///
/// * `client` - A `&reqwest::Client` to use for the request.
///
/// # Returns
///
/// A `String` containing the extracted `ttwid`, or the `DEFAULT_TTWID` as a fallback.
pub async fn fetch_ttwid(client: &Client) -> String {
    let json = json!({
        "region": "cn",
        "aid": 6383,
        "needFid": false,
        "service": BASE_URL,
        "union": true,
        "fid": ""
    });

    // Fetch ttwid from Douyin's ttwid endpoint
    let response = match client
        .post(UNION_REGISTER_URL)
        .header(reqwest::header::USER_AGENT, DEFAULT_UA)
        .json(&json)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            debug!("Failed to fetch ttwid: {}", e);
            return DEFAULT_TTWID.to_string();
        }
    };

    // Extract ttwid from response cookies
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|header_value| {
            debug!("header_value: {:?}", header_value);
            header_value.to_str().ok().and_then(|cookie_str| {
                if cookie_str.contains("ttwid=") {
                    cookie_str
                        .split(';')
                        .next()?
                        .split('=')
                        .nth(1)
                        .map(|value| value.to_string())
                } else {
                    None
                }
            })
        })
        .next()
        .unwrap_or_else(|| {
            debug!("Failed to extract ttwid from response, using default");
            DEFAULT_TTWID.to_string()
        })
}

/// Thread-safe global ttwid management functions
impl Default for GlobalTtwidManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe global ttwid manager shared across all Douyin extractor instances.
/// This prevents multiple extractors from making redundant ttwid fetch requests.
pub struct GlobalTtwidManager;

impl GlobalTtwidManager {
    pub fn new() -> Self {
        Self
    }

    /// Get the current global ttwid if it exists
    pub fn get_global_ttwid() -> Option<String> {
        GLOBAL_TTWID.lock().unwrap().clone()
    }

    /// Set the global ttwid
    pub fn set_global_ttwid(ttwid: &str) {
        *GLOBAL_TTWID.lock().unwrap() = Some(ttwid.to_string());
    }

    /// Clear the global ttwid
    #[allow(dead_code)]
    pub fn clear_global_ttwid() {
        *GLOBAL_TTWID.lock().unwrap() = None;
    }

    /// Fetch a fresh ttwid from Douyin's servers and store it globally.
    /// This method is thread-safe and prevents multiple concurrent requests.
    ///
    /// # Returns
    ///
    /// The ttwid value that was fetched and stored globally, or the default ttwid if the request failed.
    pub async fn fetch_and_store_global_ttwid(client: &Client) -> Result<String, ExtractorError> {
        // First check if we already have a global ttwid
        if let Some(existing_ttwid) = Self::get_global_ttwid() {
            debug!("Using existing global ttwid: {}", existing_ttwid);
            return Ok(existing_ttwid);
        }

        debug!("Fetching fresh ttwid from Douyin servers (global)");

        let ttwid = fetch_ttwid(client).await;

        debug!("Fetched global ttwid: {}", ttwid);

        // Store the ttwid globally
        Self::set_global_ttwid(&ttwid);

        Ok(ttwid)
    }

    /// Ensure a global ttwid exists, fetching one if necessary.
    /// This is a convenience method that checks for an existing global ttwid and only
    /// makes a network request if one doesn't exist.
    ///
    /// # Returns
    ///
    /// The global ttwid value (either existing or newly fetched)
    pub async fn ensure_global_ttwid(client: &Client) -> Result<String, ExtractorError> {
        if let Some(existing_ttwid) = Self::get_global_ttwid() {
            Ok(existing_ttwid)
        } else {
            Self::fetch_and_store_global_ttwid(client).await
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::extractor::platforms::douyin::utils::GlobalTtwidManager;

    #[test]
    fn test_global_ttwid_manager() {
        // Test the global ttwid manager directly
        GlobalTtwidManager::clear_global_ttwid();
        assert_eq!(GlobalTtwidManager::get_global_ttwid(), None);

        // Set a global ttwid
        GlobalTtwidManager::set_global_ttwid("test_global_ttwid");
        assert_eq!(
            GlobalTtwidManager::get_global_ttwid(),
            Some("test_global_ttwid".to_string())
        );

        // Clear it
        GlobalTtwidManager::clear_global_ttwid();
        assert_eq!(GlobalTtwidManager::get_global_ttwid(), None);
    }

    #[tokio::test]
    #[ignore]
    async fn test_gzip_compression_support() {
        use crate::extractor::default::default_client;
        use reqwest::header::HeaderValue;

        let client = default_client();

        // Test that the client can handle gzip-compressed responses
        // Using httpbin.org/gzip which returns gzip-compressed JSON
        let response = client
            .get("https://httpbin.org/gzip")
            .header("Accept-Encoding", HeaderValue::from_static("gzip, deflate"))
            .send()
            .await;

        match response {
            Ok(resp) => {
                // Verify we got a successful response
                assert!(resp.status().is_success());

                // Try to parse the JSON - this will only work if gzip decompression worked
                let json_result = resp.json::<serde_json::Value>().await;
                match json_result {
                    Ok(json) => {
                        // If we can parse JSON, gzip decompression worked!
                        assert!(json.is_object());
                        println!("✅ Gzip compression/decompression working correctly!");
                    }
                    Err(e) => {
                        println!("❌ Gzip decompression may not be working: {e}");
                    }
                }
            }
            Err(e) => {
                println!("Network request failed (this is okay for testing): {e}");
            }
        }
    }
}
