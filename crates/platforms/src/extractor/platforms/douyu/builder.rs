use std::sync::{Arc, LazyLock};

use async_trait::async_trait;

use parking_lot::RwLock;
use regex::Regex;
use reqwest::Client;
use reqwest::StatusCode;
use rustc_hash::FxHashMap;
use tracing::debug;

use std::collections::HashMap;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::douyu::models::{
            CachedEncryptionKey, CdnOrigin, DouyuBetardResponse, DouyuEncryptionResponse,
            DouyuH5PlayData, DouyuH5PlayResponse, DouyuInteractiveGameResponse,
            DouyuRoomInfoResponse, FallbackSignResult, ParsedStreamInfo,
        },
        utils::{extras_get_bool, extras_get_i64, extras_get_str, extras_get_u64},
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?(?:www\.)?douyu\.com/(\d+)").unwrap());

const RID_REGEX_STR: &str = r#"roomID\s*:\s*(\d+)"#;
const ROOM_STATUS_REGEX_STR: &str = r#"\$ROOM\.show_status\s*=\s*(\d+)"#;
const VIDEO_LOOP_REGEX_STR: &str = r#"videoLoop":\s*(\d+)"#;
static RID_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(RID_REGEX_STR).unwrap());
static ROOM_STATUS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ROOM_STATUS_REGEX_STR).unwrap());
static VIDEO_LOOP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VIDEO_LOOP_REGEX_STR).unwrap());

/// Regex to extract the Tencent CDN group suffix from hostname
/// Matches: sa, 3a, 1a, 3, 1 at the end of the host prefix
static TX_HOST_SUFFIX_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r".+(sa|3a|1a|3|1)").unwrap());

/// Default device ID for Douyu requests
pub const DOUYU_DEFAULT_DID: &str = "10000000000000000000000000001501";

/// Global cache for encryption key (used for fallback authentication)
static ENCRYPTION_KEY_CACHE: LazyLock<RwLock<FxHashMap<String, CachedEncryptionKey>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

fn normalize_douyu_error_body(body: &str) -> String {
    body.trim()
        .trim_matches('"')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn is_douyu_auth_failed(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::FORBIDDEN {
        return true;
    }
    let normalized = normalize_douyu_error_body(body);
    normalized.contains("鉴权失败")
}

pub struct Douyu {
    pub extractor: Extractor,
    pub cdn: String,
    /// When true, rooms running interactive games will be treated as not live
    pub disable_interactive_game: bool,
    /// Quality rate selection (0 = original quality, higher = lower quality)
    pub rate: i64,
    /// When true, force construction of hs-h5 (Huoshan) CDN URL even if API returns hs-h5
    pub force_hs: bool,
    /// Number of retries for API requests (helps with overseas/intermittent failures)
    pub request_retries: u32,
}

impl Douyu {
    const BASE_URL: &str = "https://www.douyu.com/";
    /// Default number of retries for API requests
    const DEFAULT_RETRIES: u32 = 3;

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let cdn = extras_get_str(extras.as_ref(), "cdn")
            .unwrap_or("hw-h5")
            .to_owned();

        let disable_interactive_game =
            extras_get_bool(extras.as_ref(), "disable_interactive_game").unwrap_or(false);

        let rate = extras_get_i64(extras.as_ref(), "rate").unwrap_or(0);

        let force_hs = extras_get_bool(extras.as_ref(), "force_hs").unwrap_or(false);

        let request_retries = extras_get_u64(extras.as_ref(), "request_retries")
            .map(|v| v as u32)
            .unwrap_or(Self::DEFAULT_RETRIES);

        let mut extractor = Extractor::new("Douyu", url, client);
        extractor.set_origin_and_referer_static(Self::BASE_URL);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        Self {
            extractor,
            cdn,
            disable_interactive_game,
            rate,
            force_hs,
            request_retries,
        }
    }

    pub(crate) fn extract_rid(&self, response: &str) -> Result<u64, ExtractorError> {
        let Some(captures) = RID_REGEX.captures(response) else {
            return Err(ExtractorError::ValidationError(
                "Failed to extract rid".to_string(),
            ));
        };

        let rid = captures
            .get(1)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .ok_or_else(|| ExtractorError::ValidationError("Failed to extract rid".to_string()))?;

        Ok(rid)
    }

    pub(crate) fn extract_room_status(&self, response: &str) -> Result<u64, ExtractorError> {
        let Some(captures) = ROOM_STATUS_REGEX.captures(response) else {
            return Err(ExtractorError::ValidationError(
                "Failed to extract room status".to_string(),
            ));
        };

        captures
            .get(1)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Failed to extract room status".to_string())
            })
    }

    pub(crate) fn extract_video_loop(&self, response: &str) -> Result<u32, ExtractorError> {
        let Some(captures) = VIDEO_LOOP_REGEX.captures(response) else {
            return Err(ExtractorError::ValidationError(
                "Failed to extract video loop".to_string(),
            ));
        };

        captures
            .get(1)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Failed to extract video loop".to_string())
            })
    }

    pub(crate) async fn get_web_response(&self) -> Result<String, ExtractorError> {
        let response = self.extractor.get(&self.extractor.url).send().await?;
        if !response.status().is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get web response: {}",
                response.status()
            )));
        }
        let body = response.text().await?;
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty web response".to_string(),
            ));
        }
        // debug!("Web response: {}", body);
        Ok(body)
    }

    pub(crate) async fn get_room_info(&self, rid: u64) -> Result<String, ExtractorError> {
        let url = format!("https://open.douyucdn.cn/api/RoomApi/room/{rid}");
        let response = self.extractor.get(&url).send().await?;
        let body = response.text().await.map_err(ExtractorError::from)?;
        Ok(body)
    }

    pub(crate) fn parse_room_info(
        &self,
        response: &str,
    ) -> Result<DouyuRoomInfoResponse, ExtractorError> {
        let room_info: DouyuRoomInfoResponse = serde_json::from_str(response)?;
        if room_info.error != 0 {
            return Err(ExtractorError::ValidationError(
                "Failed to parse room info".to_string(),
            ));
        }
        Ok(room_info)
    }

    /// Fetches room information from the betard API which provides VIP status
    /// This API returns more detailed room info including isVip field
    /// Includes retry logic for overseas/intermittent failures
    pub(crate) async fn get_betard_room_info(
        &self,
        rid: u64,
    ) -> Result<DouyuBetardResponse, ExtractorError> {
        let mut last_error = None;

        for attempt in 0..self.request_retries {
            match self.try_get_betard_room_info(rid).await {
                Ok(info) => return Ok(info),
                Err(e) => {
                    debug!(
                        "Betard API attempt {} failed for room {}: {}",
                        attempt + 1,
                        rid,
                        e
                    );

                    // Don't retry if the error indicates room doesn't exist (not transient)
                    if let ExtractorError::ValidationError(msg) = &e
                        && (msg.contains("没有开放") || msg.contains("不存在"))
                    {
                        return Err(e);
                    }

                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ExtractorError::ValidationError("Failed to get betard room info".to_string())
        }))
    }

    /// Single attempt to fetch betard room info
    async fn try_get_betard_room_info(
        &self,
        rid: u64,
    ) -> Result<DouyuBetardResponse, ExtractorError> {
        let url = format!("https://www.douyu.com/betard/{rid}");
        let response = self.extractor.get(&url).send().await?;

        let body = response.text().await.map_err(ExtractorError::from)?;

        // Handle empty response body (room doesn't exist)
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Betard API returned empty response (room may not exist)".to_string(),
            ));
        }

        // Handle HTML error response (room doesn't exist or is closed)
        // The API returns HTML instead of JSON when the room is not available
        if body.trim_start().starts_with('<') {
            // Try to extract error message from HTML
            let error_msg = if body.contains("该房间目前没有开放") {
                "房间目前没有开放"
            } else if body.contains("房间不存在") {
                "房间不存在"
            } else {
                "Betard API returned HTML instead of JSON"
            };
            return Err(ExtractorError::ValidationError(error_msg.to_string()));
        }

        // debug!("betard body : {}", body);
        // std::fs::write("betard.json", body.clone()).unwrap();
        let betard_info: DouyuBetardResponse = serde_json::from_str(&body).map_err(|e| {
            ExtractorError::ValidationError(format!(
                "Failed to parse betard response: {} - body: {}",
                e,
                &body[..body.len().min(200)]
            ))
        })?;

        Ok(betard_info)
    }

    /// Checks if a room is running an interactive game
    /// Interactive games are special streaming modes that may not be suitable for recording
    pub(crate) async fn has_interactive_game(&self, rid: u64) -> Result<bool, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(format!(
                "https://www.douyu.com/api/interactive/web/v2/list?rid={rid}"
            ))
            .header(reqwest::header::REFERER, Self::BASE_URL)
            .send()
            .await?;

        let body = response.text().await.map_err(ExtractorError::from)?;

        // Try to parse the response, but don't fail if it doesn't work
        match serde_json::from_str::<DouyuInteractiveGameResponse>(&body) {
            Ok(game_info) => {
                let has_game = game_info.has_interactive_game();
                if has_game {
                    debug!("Room {} has active interactive game", rid);
                }
                Ok(has_game)
            }
            Err(e) => {
                debug!(
                    "Failed to parse interactive game response for room {}: {}",
                    rid, e
                );
                // If we can't parse, assume no interactive game
                Ok(false)
            }
        }
    }

    // ==================== Mobile API ====================

    /// Mobile domain for Douyu
    const MOBILE_DOMAIN: &str = "m.douyu.com";

    /// Generates a random mobile user agent string
    fn random_mobile_user_agent() -> String {
        use rand::RngExt;
        // Common mobile user agents
        let agents = [
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (Linux; Android 13; SM-G991B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
            "Mozilla/5.0 (Linux; Android 12; Pixel 6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
        ];
        let mut rng = rand::rng();
        agents[rng.random_range(0..agents.len())].to_string()
    }

    /// Gets the real room ID from a vanity URL using the mobile domain
    /// Handles URLs like douyu.com/nickname -> actual room ID
    pub async fn get_real_room_id(&self, url_path: &str) -> Result<u64, ExtractorError> {
        // Extract the path segment (could be a number or vanity name)
        let path = url_path
            .split("douyu.com/")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.split('?').next())
            .unwrap_or("");

        // If it's already a number, return it
        if let Ok(rid) = path.parse::<u64>() {
            return Ok(rid);
        }

        // Otherwise, fetch from mobile domain to get real room ID
        let response = self
            .extractor
            .client
            .get(format!("https://{}/{}", Self::MOBILE_DOMAIN, path))
            .header(
                reqwest::header::USER_AGENT,
                Self::random_mobile_user_agent(),
            )
            .send()
            .await?;

        let body = response.text().await.map_err(ExtractorError::from)?;

        // Look for roomInfo":{"rid":(\d+) pattern
        static REAL_RID_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"roomInfo":\{"rid":(\d+)"#).unwrap());

        if let Some(captures) = REAL_RID_REGEX.captures(&body)
            && let Some(rid_match) = captures.get(1)
        {
            return rid_match
                .as_str()
                .parse::<u64>()
                .map_err(|_| ExtractorError::ValidationError("Invalid room ID".to_string()));
        }

        Err(ExtractorError::ValidationError(format!(
            "Could not resolve real room ID for path: {}",
            path
        )))
    }

    // ==================== Fallback Authentication ====================

    /// Generates a random desktop user agent string
    fn random_desktop_user_agent() -> String {
        use rand::RngExt;
        let agents = [
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:121.0) Gecko/20100101 Firefox/121.0",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        ];
        let mut rng = rand::rng();
        agents[rng.random_range(0..agents.len())].to_string()
    }

    /// Fetches encryption key from Douyu API for fallback authentication
    /// This allows signing requests without a JS engine
    async fn fetch_encryption_key(&self, did: &str) -> Result<CachedEncryptionKey, ExtractorError> {
        let user_agent = Self::random_desktop_user_agent();

        let response = self
            .extractor
            .client
            .get("https://www.douyu.com/wgapi/livenc/liveweb/websec/getEncryption")
            .query(&[("did", did)])
            .header(reqwest::header::USER_AGENT, &user_agent)
            .header(reqwest::header::REFERER, Self::BASE_URL)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await.map_err(ExtractorError::from)?;
        debug!(
            "Encryption key response (status {}): {}",
            status,
            &body[..body.len().min(500)]
        );

        let enc_response: DouyuEncryptionResponse = serde_json::from_str(&body).map_err(|e| {
            ExtractorError::ValidationError(format!(
                "Failed to parse encryption response: {} - body: {}",
                e,
                &body[..body.len().min(500)]
            ))
        })?;

        if enc_response.error != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Encryption API error: code={}, msg={}",
                enc_response.error, enc_response.msg
            )));
        }

        let data = enc_response.data.ok_or_else(|| {
            ExtractorError::ValidationError("Encryption API returned no data".to_string())
        })?;

        debug!(
            "Encryption key fetched: rand_str={}, enc_time={}, is_special={}",
            data.rand_str, data.enc_time, data.is_special
        );

        Ok(CachedEncryptionKey::new(data, user_agent))
    }

    /// Gets a valid encryption key, fetching a new one if needed
    /// Uses a global cache to avoid repeated API calls
    async fn get_encryption_key(&self, did: &str) -> Result<CachedEncryptionKey, ExtractorError> {
        {
            let cache = ENCRYPTION_KEY_CACHE.read();
            if let Some(cached) = cache.get(did)
                && cached.is_valid()
            {
                return Ok(cached.clone());
            }
        }

        {
            let mut cache = ENCRYPTION_KEY_CACHE.write();
            cache.retain(|_, v| v.is_valid());
            if let Some(cached) = cache.get(did) {
                return Ok(cached.clone());
            }
        }

        let new_key = self.fetch_encryption_key(did).await?;

        {
            let mut cache = ENCRYPTION_KEY_CACHE.write();
            cache.retain(|_, v| v.is_valid());
            cache.insert(did.to_string(), new_key.clone());
        }

        Ok(new_key)
    }

    fn invalidate_encryption_key(did: &str) {
        ENCRYPTION_KEY_CACHE.write().remove(did);
    }

    /// Generates authentication signature using fallback method (no JS engine required)
    /// This implements the DouyuUtils.sign algorithm from the Python version
    ///
    /// # Arguments
    /// * `rid` - Room ID
    /// * `did` - Device ID (defaults to DOUYU_DEFAULT_DID)
    /// * `ts` - Timestamp (defaults to current time)
    pub async fn fallback_sign(
        &self,
        rid: u64,
        did: Option<&str>,
        ts: Option<u64>,
    ) -> Result<FallbackSignResult, ExtractorError> {
        let did = did.unwrap_or(DOUYU_DEFAULT_DID);

        Ok(self.fallback_sign_with_key(rid, did, ts).await?.0)
    }

    async fn fallback_sign_with_key(
        &self,
        rid: u64,
        did: &str,
        ts: Option<u64>,
    ) -> Result<(FallbackSignResult, CachedEncryptionKey), ExtractorError> {
        use md5::{Digest, Md5};

        let ts = ts.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });

        let key_data = self.get_encryption_key(did).await?;
        let enc = &key_data.data;

        // Generate secret through iterative MD5 hashing
        let mut secret = enc.rand_str.clone();
        for _ in 0..enc.enc_time {
            let mut hasher = Md5::new();
            hasher.update(format!("{}{}", secret, enc.key).as_bytes());
            secret = format!("{:x}", hasher.finalize());
        }

        // Generate salt (empty if is_special, otherwise rid+ts)
        let salt = if enc.is_special {
            String::new()
        } else {
            format!("{}{}", rid, ts)
        };

        // Generate final auth signature
        let mut hasher = Md5::new();
        hasher.update(format!("{}{}{}", secret, enc.key, salt).as_bytes());
        let auth = format!("{:x}", hasher.finalize());

        Ok((
            FallbackSignResult {
                auth,
                ts,
                enc_data: enc.enc_data.clone(),
            },
            key_data,
        ))
    }

    /// Gets play info using server-side authentication (V1 API)
    /// This method doesn't require a JS engine
    ///
    /// # Arguments
    /// * `rid` - Room ID
    /// * `cdn` - CDN to request
    /// * `rate` - Quality rate (0 = original)
    /// * `did` - Device ID (optional, defaults to DOUYU_DEFAULT_DID)
    pub async fn get_play_info_fallback(
        &self,
        rid: u64,
        cdn: &str,
        rate: i64,
        did: Option<&str>,
    ) -> Result<DouyuH5PlayData, ExtractorError> {
        let did = did.unwrap_or(DOUYU_DEFAULT_DID);

        // If auth fails, refresh encryption key once and retry (Python tends to refresh often).
        for attempt in 0..2 {
            if attempt == 1 {
                debug!(
                    "Retrying getH5PlayV1 after auth failure by refreshing encryption key (rid={}, did={})",
                    rid, did
                );
                Self::invalidate_encryption_key(did);
            }

            let (sign_result, key_data) = self.fallback_sign_with_key(rid, did, None).await?;

            let mut form_data: HashMap<&str, String> = HashMap::new();
            form_data.insert("enc_data", sign_result.enc_data);
            form_data.insert("tt", sign_result.ts.to_string());
            form_data.insert("did", did.to_string());
            form_data.insert("auth", sign_result.auth);
            form_data.insert("cdn", cdn.to_string());
            form_data.insert("rate", rate.to_string());
            form_data.insert("ver", "219032101".to_string());
            form_data.insert("iar", "0".to_string());
            form_data.insert("ive", "0".to_string());
            form_data.insert("rid", rid.to_string());
            form_data.insert("hevc", "0".to_string());
            form_data.insert("fa", "0".to_string());
            form_data.insert("sov", "0".to_string());

            // Fallback auth always uses V1 API with POST
            debug!(
                "Requesting getH5PlayV1 with auth={}, ts={}, did={}",
                form_data.get("auth").unwrap_or(&String::new()),
                form_data.get("tt").unwrap_or(&String::new()),
                did
            );

            let api_response = self
                .extractor
                .client
                .post(format!("https://www.douyu.com/lapi/live/getH5PlayV1/{rid}"))
                .header(reqwest::header::USER_AGENT, &key_data.user_agent)
                .header(reqwest::header::REFERER, Self::BASE_URL)
                .query(&form_data)
                .form(&form_data)
                .send()
                .await?;

            let status = api_response.status();
            let body = api_response.text().await?;
            debug!("getH5PlayV1 response (status {}): {}", status, body);

            if is_douyu_auth_failed(status, &body) {
                if attempt == 0 {
                    continue;
                }
                return Err(ExtractorError::ValidationError(format!(
                    "getH5PlayV1 auth failed (status {}): {}",
                    status,
                    normalize_douyu_error_body(&body)
                )));
            }

            let resp: DouyuH5PlayResponse = serde_json::from_str(&body).map_err(|e| {
                ExtractorError::ValidationError(format!(
                    "Failed to parse getH5PlayV1 response: {} - body: {}",
                    e,
                    &body[..body.len().min(500)]
                ))
            })?;

            if resp.error != 0 {
                // Some failures are auth-related but returned as JSON.
                if is_douyu_auth_failed(status, &resp.msg) && attempt == 0 {
                    continue;
                }
                return Err(ExtractorError::ValidationError(format!(
                    "Failed to get play info (fallback): {}",
                    resp.msg
                )));
            }

            return resp.data.ok_or_else(|| {
                ExtractorError::ValidationError(
                    "Failed to get play info (fallback): no data".to_string(),
                )
            });
        }

        Err(ExtractorError::ValidationError(
            "getH5PlayV1 auth failed after retry".to_string(),
        ))
    }

    // ==================== CDN URL Construction ====================

    /// Parses a Douyu stream URL into its components
    /// Returns the Tencent app name, stream ID, and query parameters
    pub fn parse_stream_url(url: &str, rid: u64) -> Result<ParsedStreamInfo, ExtractorError> {
        // Split URL into base and query string
        let parts: Vec<&str> = url.splitn(2, '?').collect();
        let base_url = parts[0];
        let query_string = parts.get(1).unwrap_or(&"");

        // Parse query parameters
        let mut query_params: HashMap<String, String> = HashMap::new();
        for (k, v) in url::form_urlencoded::parse(query_string.as_bytes()) {
            query_params.insert(k.into_owned(), v.into_owned());
        }

        // Extract host from URL
        let host = base_url
            .split("//")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .unwrap_or("")
            .to_string();

        // Extract stream ID similar to Python: /({rid}[^\._/]+)
        let stream_id = Self::extract_stream_id_from_base_url(base_url, rid).unwrap_or_else(|| {
            base_url
                .split('/')
                .next_back()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("")
                .split('_')
                .next()
                .unwrap_or("")
                .to_string()
        });

        // Get app name from the path prefix (like Python get_tx_app_name)
        let prefix = base_url
            .split(&stream_id)
            .next()
            .unwrap_or(base_url)
            .trim_end_matches('/');
        let app_name = prefix.split('/').next_back().unwrap_or("");

        // Get Tencent app name from host (suffix mapping) or fallback to app name
        let tx_app_name = Self::get_tx_app_name(&host, app_name);

        Ok(ParsedStreamInfo {
            tx_app_name,
            stream_id,
            query_params,
            host,
        })
    }

    fn extract_stream_id_from_base_url(base_url: &str, rid: u64) -> Option<String> {
        let rid_str = rid.to_string();
        let marker = format!("/{rid_str}");
        let pos = base_url.find(&marker)?;
        let start = pos + 1; // skip '/'
        let rest = base_url.get(start..)?;
        let end = rest
            .find(|c: char| ['.', '_', '/'].contains(&c))
            .unwrap_or(rest.len());
        let out = rest.get(..end)?.to_string();
        if out.is_empty() { None } else { Some(out) }
    }

    /// Gets the Tencent Cloud app name from the RTMP URL host
    /// Maps host suffixes to dyliveflv app names
    fn get_tx_app_name(host: &str, app_name: &str) -> String {
        if let Some(captures) = TX_HOST_SUFFIX_REGEX.captures(host)
            && let Some(suffix) = captures.get(1)
        {
            let suffix_str = suffix.as_str();
            // "sa" maps to "1"
            let num = if suffix_str == "sa" { "1" } else { suffix_str };
            return format!("dyliveflv{}", num);
        }
        if app_name.is_empty() {
            "dyliveflv1".to_string()
        } else {
            app_name.to_string()
        }
    }

    /// Builds a Tencent CDN URL from the current stream info.
    /// Mirrors the direct conversion path in the old builder.
    fn build_tencent_url(
        stream_info: &ParsedStreamInfo,
        additional_params: Option<&HashMap<String, String>>,
    ) -> Result<String, ExtractorError> {
        let origin = stream_info
            .query_params
            .get("origin")
            .map(|s| CdnOrigin::from_str(s))
            .unwrap_or(CdnOrigin::Unknown);

        match origin {
            CdnOrigin::Unknown => {
                return Err(ExtractorError::ValidationError(format!(
                    "Unknown origin '{}' cannot be converted to Tencent CDN",
                    stream_info
                        .query_params
                        .get("origin")
                        .unwrap_or(&"".to_string())
                )));
            }
            CdnOrigin::Douyu => {
                debug!("Origin is Douyu self-built, Tencent stream may not exist");
            }
            _ => {}
        }

        let tx_host = "tc-tct.douyucdn2.cn";
        let mut query = stream_info.query_params.clone();
        query.insert("fcdn".to_string(), "tct".to_string());

        if let Some(params) = additional_params {
            for (k, v) in params {
                query.insert(k.clone(), v.clone());
            }
        }

        query.remove("vhost");

        let query_string = Self::encode_query_params(&query);
        Ok(format!(
            "https://{}/{}/{}.flv?{}",
            tx_host, stream_info.tx_app_name, stream_info.stream_id, query_string
        ))
    }

    /// Builds a Huoshan/Volcano (hs-h5) URL from a Tencent URL.
    fn build_huoshan_url(
        stream_info: &ParsedStreamInfo,
        tencent_url: &str,
    ) -> Result<(String, String), ExtractorError> {
        let tx_host = tencent_url
            .split("//")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .unwrap_or("tc-tct.douyucdn2.cn");

        let hs_host = format!(
            "{}.douyucdn2.cn",
            stream_info.tx_app_name.replace("dyliveflv", "huos")
        )
        .replace("huos1.", "huosa.");

        let mut query = stream_info.query_params.clone();
        let encoded_url =
            percent_encoding::utf8_percent_encode(tencent_url, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        query.insert("fp_user_url".to_string(), encoded_url);
        query.insert("vhost".to_string(), tx_host.to_string());
        query.insert("domain".to_string(), tx_host.to_string());

        let query_string = Self::encode_query_params(&query);
        let hs_cname_url = format!(
            "http://douyu-pull.s.volcfcdndvs.com/live/{}.flv?{}",
            stream_info.stream_id, query_string
        );

        Ok((hs_host, hs_cname_url))
    }

    /// Encodes query parameters into a URL query string
    fn encode_query_params(params: &HashMap<String, String>) -> String {
        let mut keys: Vec<&String> = params.keys().collect();
        keys.sort_unstable();
        keys.into_iter()
            .map(|k| {
                let v = params.get(k).map(String::as_str).unwrap_or_default();
                format!(
                    "{}={}",
                    k,
                    percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)
                )
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Checks if a CDN type starts with "scdn" (problematic CDN to avoid)
    pub fn is_scdn(cdn: &str) -> bool {
        cdn.starts_with("scdn")
    }

    #[allow(clippy::too_many_arguments)]
    fn create_media_info(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<String>,
        avatar_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
        rid: Option<u64>,
    ) -> MediaInfo {
        let extras = rid.map(|r| {
            let mut map = FxHashMap::default();
            map.insert("rid".to_string(), r.to_string());
            map
        });
        MediaInfo::new(
            self.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
            extras,
        )
    }

    pub(crate) async fn parse_web_response(
        &self,
        response: Arc<str>,
    ) -> Result<MediaInfo, ExtractorError> {
        if response.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty response".to_string(),
            ));
        }
        if response.contains("该房间目前没有开放") || response.contains("房间不存在")
        {
            return Err(ExtractorError::StreamerNotFound);
        }

        if response.contains("房间已被关闭") {
            return Ok(self.create_media_info("Douyu", "", None, None, false, vec![], None));
        }

        let rid = match self.extract_rid(&response) {
            Ok(rid) => rid,
            Err(_) => {
                // If RID extraction fails, try to resolve vanity URL
                debug!("Failed to extract RID from HTML, trying vanity URL resolution");
                self.get_real_room_id(&self.extractor.url).await?
            }
        };

        // Prefer checking live status from HTML first (avoid calling betard for
        // offline/loop rooms). If parsing fails, we'll fall back to betard.
        let live_from_html = match (
            self.extract_room_status(&response),
            self.extract_video_loop(&response),
        ) {
            (Ok(room_status), Ok(video_loop)) => Some(room_status == 1 && video_loop == 0),
            _ => None,
        };

        if let Some(false) = live_from_html {
            // Not live (or is playing a recording loop). Try RoomApi for metadata,
            // but don't fail extraction if it errors.
            let mut title = "Douyu".to_string();
            let mut artist = String::new();
            let mut cover_url = None;
            let mut avatar_url = None;

            match self
                .get_room_info(rid)
                .await
                .and_then(|body| self.parse_room_info(&body))
            {
                Ok(room_info) => {
                    title = room_info.data.room_name;
                    artist = room_info.data.owner_name;
                    cover_url = Some(room_info.data.room_thumb);
                    avatar_url = Some(room_info.data.avatar);
                }
                Err(e) => {
                    debug!("Failed to fetch RoomApi metadata for {}: {}", rid, e);
                }
            }

            return Ok(self.create_media_info(
                &title,
                &artist,
                cover_url,
                avatar_url,
                false,
                vec![],
                Some(rid),
            ));
        }

        // Use betard API for more reliable room info including VIP status
        let betard_info = self.get_betard_room_info(rid).await;
        // debug!("betard_info: {:#?}", betard_info);

        // Determine live status - prefer betard API, fallback to HTML parsing
        let (is_live, is_vip, title, artist, cover_url, avatar_url) = match betard_info {
            Ok(info) => {
                let room = &info.room;
                let live = room.show_status == 1 && room.video_loop == 0;
                let vip = room.is_vip == 1;

                if vip {
                    debug!("Room {} is a VIP room", rid);
                }

                (
                    live,
                    vip,
                    room.room_name.clone(),
                    room.owner_name.clone(),
                    if room.room_thumb.is_empty() {
                        None
                    } else {
                        Some(room.room_thumb.clone())
                    },
                    room.avatar.get_best(),
                )
            }
            Err(e) => {
                debug!("Betard API failed, falling back: {}", e);

                let live = live_from_html.unwrap_or(true);

                match self
                    .get_room_info(rid)
                    .await
                    .and_then(|body| self.parse_room_info(&body))
                {
                    Ok(room_info) => (
                        live,
                        false, // Cannot determine VIP status without betard API
                        room_info.data.room_name,
                        room_info.data.owner_name,
                        Some(room_info.data.room_thumb),
                        Some(room_info.data.avatar),
                    ),
                    Err(e) => {
                        debug!("RoomApi failed for {}: {}", rid, e);
                        (live, false, "Douyu".to_string(), String::new(), None, None)
                    }
                }
            }
        };

        if !is_live {
            return Ok(self.create_media_info(
                &title,
                &artist,
                cover_url,
                avatar_url,
                false,
                vec![],
                Some(rid),
            ));
        }

        // Check for interactive game if filtering is enabled
        if self.disable_interactive_game {
            match self.has_interactive_game(rid).await {
                Ok(true) => {
                    debug!(
                        "Room {} is running an interactive game, treating as not live",
                        rid
                    );
                    return Ok(self.create_media_info(
                        &title,
                        &artist,
                        cover_url,
                        avatar_url,
                        false,
                        vec![],
                        Some(rid),
                    ));
                }
                Ok(false) => {
                    // No interactive game, continue
                }
                Err(e) => {
                    // Log the error but continue - don't fail the whole extraction
                    debug!("Failed to check interactive game status: {}", e);
                }
            }
        }

        // streamer is live
        let streams = self.get_streams_with_stable_auth(rid, is_vip).await?;

        Ok(self.create_media_info(
            &title,
            &artist,
            cover_url,
            avatar_url,
            true,
            streams,
            Some(rid),
        ))
    }

    /// Gets streams using stable server-side authentication
    async fn get_streams_with_stable_auth(
        &self,
        rid: u64,
        _is_vip: bool,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut stream_infos = vec![];

        // Use stable server-side authentication to get play info (with scdn avoidance)
        let (data, actual_cdn) = self
            .get_play_info_fallback_with_scdn_avoidance(rid, &self.cdn, self.rate, None)
            .await?;

        // Prepare the list of CDNs to process
        let mut cdns_to_process = data.cdns.clone();

        // Always offer hs-h5 (Huoshan) as an option, even if it's not returned by the API.
        // It can be resolved later in `get_url` by constructing the URL manually.
        let want_hs = self.cdn == "hs-h5";
        if !cdns_to_process.iter().any(|c| c.cdn == "hs-h5") {
            // Create a synthetic hs-h5 entry (resolved in get_url).
            let is_h265 = cdns_to_process
                .iter()
                .find(|c| c.cdn == "tct-h5")
                .map(|c| c.is_h265)
                .unwrap_or(false);

            cdns_to_process.push(crate::extractor::platforms::douyu::models::CdnsWithName {
                name: "火山".to_string(),
                cdn: "hs-h5".to_string(),
                is_h265,
                re_weight: None,
            });
        }

        let preferred_cdn = if want_hs {
            "hs-h5"
        } else {
            actual_cdn.as_str()
        };
        let preferred_rate = self.rate as u64;

        for cdn in cdns_to_process {
            for rate in &data.multirates {
                let stream_url = "".to_string();

                let format = if data.rtmp_live.contains("flv") {
                    StreamFormat::Flv
                } else {
                    StreamFormat::Hls
                };
                let media_format = if data.rtmp_live.contains("flv") {
                    MediaFormat::Flv
                } else {
                    MediaFormat::Ts
                };

                let codec = if cdn.is_h265 { "hevc,aac" } else { "avc,aac" };

                let priority = if cdn.cdn == preferred_cdn && rate.rate == preferred_rate {
                    0
                } else {
                    10
                };

                let extras = serde_json::json!({
                    "cdn": cdn.cdn.clone(),
                    "rate": rate.rate.to_string(),
                    "rid": rid.to_string(),
                });

                stream_infos.push(
                    StreamInfo::builder(stream_url, format, media_format)
                        .quality(rate.name.to_string())
                        .bitrate(rate.bit)
                        .priority(priority)
                        .extras(extras)
                        .codec(codec.to_string())
                        .is_headers_needed(true)
                        .build(),
                );
            }
        }

        Ok(stream_infos)
    }

    /// Maximum number of retries when avoiding scdn
    const MAX_SCDN_RETRIES: u32 = 2;

    /// Gets play info (fallback auth) with automatic scdn avoidance.
    /// If the returned CDN starts with "scdn", it will retry with a fallback CDN from the list.
    async fn get_play_info_fallback_with_scdn_avoidance(
        &self,
        rid: u64,
        cdn: &str,
        rate: i64,
        did: Option<&str>,
    ) -> Result<(DouyuH5PlayData, String), ExtractorError> {
        let mut current_cdn = cdn.to_string();

        for attempt in 0..Self::MAX_SCDN_RETRIES {
            let resp = self
                .get_play_info_fallback(rid, &current_cdn, rate, did)
                .await?;

            if Self::is_scdn(&resp.rtmp_cdn) {
                debug!(
                    "Attempt {}: Got scdn '{}' (requested '{}'), trying to avoid",
                    attempt + 1,
                    resp.rtmp_cdn,
                    current_cdn
                );

                if let Some(fallback) = Self::find_non_scdn_fallback(&resp.cdns) {
                    debug!("Switching from scdn to fallback CDN: {}", fallback);
                    current_cdn = fallback;
                    continue;
                }

                debug!("No non-scdn fallback available, using scdn");
                return Ok((resp, current_cdn));
            }

            debug!("Using CDN: {} (rtmp_cdn: {})", current_cdn, resp.rtmp_cdn);
            return Ok((resp, current_cdn));
        }

        let resp = self
            .get_play_info_fallback(rid, &current_cdn, rate, did)
            .await?;
        Ok((resp, current_cdn))
    }

    /// Finds a non-scdn fallback CDN from the available CDN list
    /// Prefers the last CDN in the list
    fn find_non_scdn_fallback(
        cdns: &[crate::extractor::platforms::douyu::models::CdnsWithName],
    ) -> Option<String> {
        // First, try to find any non-scdn CDN from the end of the list
        for cdn in cdns.iter().rev() {
            if !Self::is_scdn(&cdn.cdn) {
                return Some(cdn.cdn.clone());
            }
        }
        // If all are scdn, return the last one anyway
        cdns.last().map(|c| c.cdn.clone())
    }
}

#[async_trait]
impl PlatformExtractor for Douyu {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let response = self.get_web_response().await?;
        let response_arc: Arc<str> = response.into();
        let media_info = self.parse_web_response(response_arc).await?;
        Ok(media_info)
    }

    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        if !stream_info.url.is_empty() {
            return Ok(());
        }

        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Missing extras in stream info".to_string())
        })?;

        let rid = extras["rid"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| ExtractorError::ValidationError("Missing rid in extras".to_string()))?;

        let cdn = extras["cdn"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing cdn in extras".to_string()))?;

        let rate = extras["rate"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| ExtractorError::ValidationError("Missing rate in extras".to_string()))?;

        debug!("Resolving Douyu stream URL for rid: {}", rid);
        let (resp, _actual_cdn) = self
            .get_play_info_fallback_with_scdn_avoidance(rid, cdn, rate, None)
            .await?;

        let base_stream_url = format!("{}/{}", resp.rtmp_url, resp.rtmp_live);

        if cdn == "hs-h5" {
            let need_build = self.force_hs || resp.rtmp_cdn != "hs-h5";
            if need_build {
                let stream_info_parsed = match Self::parse_stream_url(&base_stream_url, rid) {
                    Ok(info) => info,
                    Err(e) => {
                        debug!("Failed to parse stream URL: {} (fallback to base)", e);
                        stream_info.url = base_stream_url;
                        return Ok(());
                    }
                };

                let tencent_url = match Self::build_tencent_url(&stream_info_parsed, None) {
                    Ok(url) => url,
                    Err(e) => {
                        debug!("Failed to build Tencent URL: {} (fallback to base)", e);
                        stream_info.url = base_stream_url;
                        return Ok(());
                    }
                };

                match Self::build_huoshan_url(&stream_info_parsed, &tencent_url) {
                    Ok((host, url)) => {
                        stream_info.url = url;

                        let extras_value = stream_info
                            .extras
                            .get_or_insert_with(|| serde_json::json!({}));
                        if !extras_value.is_object() {
                            *extras_value = serde_json::json!({});
                        }
                        let Some(extras_obj) = extras_value.as_object_mut() else {
                            // We just enforced object above; if serde_json breaks that invariant,
                            // skip header injection rather than panic.
                            return Ok(());
                        };

                        // Backward-compat (older code used host_header string).
                        extras_obj.insert(
                            "host_header".to_string(),
                            serde_json::Value::String(host.clone()),
                        );

                        let headers_value = extras_obj
                            .entry("headers".to_string())
                            .or_insert_with(|| serde_json::json!({}));
                        if !headers_value.is_object() {
                            *headers_value = serde_json::json!({});
                        }
                        if let Some(headers_obj) = headers_value.as_object_mut() {
                            headers_obj.insert("Host".to_string(), serde_json::Value::String(host));
                        }
                    }
                    Err(e) => {
                        debug!("Failed to build hs-h5 URL (fallback to base): {}", e);
                        stream_info.url = base_stream_url;
                    }
                }
            } else {
                stream_info.url = base_stream_url;
            }
        } else {
            stream_info.url = base_stream_url;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor, platforms::douyu::Douyu,
    };

    #[tokio::test]
    #[ignore]
    async fn test_douyu_extractor() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .try_init()
            .unwrap();

        let url = "https://www.douyu.com/309763";

        let extractor = Douyu::new(url.to_string(), default_client(), None, None);
        let media_info = extractor.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
