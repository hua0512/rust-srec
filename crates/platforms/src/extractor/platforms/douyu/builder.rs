use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use boa_engine::{self, property::PropertyKey};
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tokio::task;
use tracing::debug;
use uuid::Uuid;

use std::collections::HashMap;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::douyu::models::{
            CachedEncryptionKey, CdnOrigin, DouyuBetardResponse, DouyuEncryptionResponse,
            DouyuH5PlayData, DouyuH5PlayResponse, DouyuInteractiveGameResponse,
            DouyuMobilePlayData, DouyuMobilePlayResponse, DouyuRoomInfoResponse,
            FallbackSignResult, ParsedStreamInfo,
        },
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

use std::sync::RwLock;

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?(?:www\.)?douyu\.com/(\d+)").unwrap());

const RID_REGEX_STR: &str = r#"\$ROOM\.room_id\s*=\s*(\d+)"#;
const ROOM_STATUS_REGEX_STR: &str = r#"\$ROOM\.show_status\s*=\s*(\d+)"#;
const VIDEO_LOOP_REGEX_STR: &str = r#"videoLoop":\s*(\d+)"#;
static RID_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(RID_REGEX_STR).unwrap());
static ROOM_STATUS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ROOM_STATUS_REGEX_STR).unwrap());
static VIDEO_LOOP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VIDEO_LOOP_REGEX_STR).unwrap());

const ENCODED_SCRIPT_REGEX_STR: &str = r#"(var vdwdae325w_64we =[\s\S]+?)\s*</script>"#;
static ENCODED_SCRIPT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ENCODED_SCRIPT_REGEX_STR).unwrap());
static SIGN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v=(\d+)&did=(\w+)&tt=(\d+)&sign=(\w+)").unwrap());

/// Regex to extract the Tencent CDN group suffix from hostname
/// Matches: sa, 3a, 1a, 3, 1 at the end of the host prefix
static TX_HOST_SUFFIX_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r".+(sa|3a|1a|3|1)").unwrap());

/// Default device ID for Douyu requests
pub const DOUYU_DEFAULT_DID: &str = "10000000000000000000000000001501";

/// Global cache for encryption key (used for fallback authentication)
static ENCRYPTION_KEY_CACHE: LazyLock<RwLock<Option<CachedEncryptionKey>>> =
    LazyLock::new(|| RwLock::new(None));

struct DouyuTokenResult {
    v: String,
    did: String,
    tt: String,
    sign: String,
}

impl DouyuTokenResult {
    pub fn new(v: &str, did: &str, tt: &str, sign: &str) -> Self {
        Self {
            v: v.to_string(),
            did: did.to_string(),
            tt: tt.to_string(),
            sign: sign.to_string(),
        }
    }
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
        let cdn = extras
            .as_ref()
            .and_then(|extras| extras.get("cdn").and_then(|v| v.as_str()))
            .unwrap_or("ws-h5")
            .to_string();

        let disable_interactive_game = extras
            .as_ref()
            .and_then(|extras| {
                extras
                    .get("disable_interactive_game")
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false);

        let rate = extras
            .as_ref()
            .and_then(|extras| extras.get("rate").and_then(|v| v.as_i64()))
            .unwrap_or(0);

        let force_hs = extras
            .as_ref()
            .and_then(|extras| extras.get("force_hs").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        let request_retries = extras
            .as_ref()
            .and_then(|extras| extras.get("request_retries").and_then(|v| v.as_u64()))
            .map(|v| v as u32)
            .unwrap_or(Self::DEFAULT_RETRIES);

        let mut extractor = Extractor::new("Douyu".to_string(), url, client);

        extractor.add_header(
            reqwest::header::ORIGIN.to_string(),
            Self::BASE_URL.to_string(),
        );

        extractor.add_header(
            reqwest::header::REFERER.to_string(),
            Self::BASE_URL.to_string(),
        );

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
        let captures = RID_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u64>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract rid".to_string(),
        ))
    }

    pub(crate) fn extract_room_status(&self, response: &str) -> Result<u64, ExtractorError> {
        let captures = ROOM_STATUS_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u64>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract room status".to_string(),
        ))
    }

    pub(crate) fn extract_video_loop(&self, response: &str) -> Result<u32, ExtractorError> {
        let captures = VIDEO_LOOP_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u32>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract video loop".to_string(),
        ))
    }

    pub(crate) async fn get_web_response(&self) -> Result<String, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(&self.extractor.url)
            .send()
            .await?;
        let body = response.text().await.map_err(ExtractorError::from)?;
        Ok(body)
    }

    pub(crate) async fn get_room_info(&self, rid: u64) -> Result<String, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(format!("https://open.douyucdn.cn/api/RoomApi/room/{rid}"))
            .send()
            .await?;
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
        let response = self
            .extractor
            .client
            .get(format!("https://www.douyu.com/betard/{rid}"))
            .header(reqwest::header::REFERER, Self::BASE_URL)
            .send()
            .await?;

        let body = response.text().await.map_err(ExtractorError::from)?;
        // debug!("betard body : {}", body);
        let betard_info: DouyuBetardResponse = serde_json::from_str(&body).map_err(|e| {
            ExtractorError::ValidationError(format!("Failed to parse betard response: {}", e))
        })?;

        Ok(betard_info)
    }

    /// Checks if a room is a VIP room using the betard API
    /// VIP rooms may have different API handling requirements
    #[allow(dead_code)]
    pub(crate) async fn is_vip_room(&self, rid: u64) -> Result<bool, ExtractorError> {
        let betard_info = self.get_betard_room_info(rid).await?;
        Ok(betard_info.room.is_vip == 1)
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
        use rand::Rng;
        // Common mobile user agents
        let agents = [
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (Linux; Android 13; SM-G991B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
            "Mozilla/5.0 (Linux; Android 12; Pixel 6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
        ];
        let mut rng = rand::rng();
        agents[rng.random_range(0..agents.len())].to_string()
    }

    /// Gets play info from the mobile API
    /// Mobile tokens have looser validation and are useful for CDN switching
    ///
    /// # Arguments
    /// * `token` - Token result from JS signing (contains v, did, tt, sign)
    /// * `rid` - Room ID
    /// * `cdn` - CDN to request
    /// * `rate` - Quality rate (0 = original)
    #[allow(clippy::too_many_arguments)]
    pub async fn get_mobile_play_info(
        &self,
        v: &str,
        did: &str,
        tt: &str,
        sign: &str,
        rid: u64,
        cdn: &str,
        rate: i64,
    ) -> Result<DouyuMobilePlayData, ExtractorError> {
        let mut form_data: FxHashMap<&str, String> = FxHashMap::default();
        form_data.insert("v", v.to_string());
        form_data.insert("did", did.to_string());
        form_data.insert("tt", tt.to_string());
        form_data.insert("sign", sign.to_string());
        form_data.insert("cdn", cdn.to_string());
        form_data.insert("rate", rate.to_string());
        form_data.insert("rid", rid.to_string());

        let resp = self
            .extractor
            .client
            .post(format!(
                "https://{}/api/room/ratestream",
                Self::MOBILE_DOMAIN
            ))
            .header(
                reqwest::header::USER_AGENT,
                Self::random_mobile_user_agent(),
            )
            .header(
                reqwest::header::REFERER,
                format!("https://{}/", Self::MOBILE_DOMAIN),
            )
            .form(&form_data)
            .send()
            .await?
            .json::<DouyuMobilePlayResponse>()
            .await?;

        if resp.code != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get mobile play info: code={}, msg={}",
                resp.code, resp.msg
            )));
        }

        resp.data.ok_or_else(|| {
            ExtractorError::ValidationError("Failed to get mobile play info: no data".to_string())
        })
    }

    /// Parses query parameters from a mobile stream URL
    /// Returns a HashMap of the query parameters
    pub fn parse_mobile_stream_params(url: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();

        if let Some(query_start) = url.find('?') {
            let query_string = &url[query_start + 1..];
            for pair in query_string.split('&') {
                if let Some((key, value)) = pair.split_once('=') {
                    params.insert(
                        key.to_string(),
                        urlencoding::decode(value).unwrap_or_default().to_string(),
                    );
                }
            }
        }

        params
    }

    /// Builds a Tencent CDN URL using mobile API tokens
    /// Mobile tokens have looser validation, making CDN switching more reliable
    ///
    /// # Arguments
    /// * `stream_info` - Parsed stream info from the original URL
    /// * `mobile_params` - Query parameters from mobile API response
    pub fn build_tencent_url_with_mobile_token(
        stream_info: &ParsedStreamInfo,
        mobile_params: &HashMap<String, String>,
    ) -> Result<String, ExtractorError> {
        let tx_host = "tc-tct.douyucdn2.cn";
        let mut query = stream_info.query_params.clone();

        // Add mobile params (they have looser validation)
        for (k, v) in mobile_params {
            if k != "vhost" {
                // Don't copy vhost from mobile
                query.insert(k.clone(), v.clone());
            }
        }

        query.insert("fcdn".to_string(), "tct".to_string());
        query.remove("vhost");

        let query_string = Self::encode_query_params(&query);
        Ok(format!(
            "https://{}/{}/{}.flv?{}",
            tx_host, stream_info.tx_app_name, stream_info.stream_id, query_string
        ))
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
        use rand::Rng;
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

        let body = response.text().await.map_err(ExtractorError::from)?;
        let enc_response: DouyuEncryptionResponse = serde_json::from_str(&body).map_err(|e| {
            ExtractorError::ValidationError(format!("Failed to parse encryption response: {}", e))
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

        Ok(CachedEncryptionKey::new(data, user_agent))
    }

    /// Gets a valid encryption key, fetching a new one if needed
    /// Uses a global cache to avoid repeated API calls
    async fn get_encryption_key(&self, did: &str) -> Result<CachedEncryptionKey, ExtractorError> {
        // Check cache first
        {
            let cache = ENCRYPTION_KEY_CACHE.read().unwrap();
            if let Some(ref cached) = *cache
                && cached.is_valid()
            {
                return Ok(cached.clone());
            }
        }

        // Fetch new key
        let new_key = self.fetch_encryption_key(did).await?;

        // Update cache
        {
            let mut cache = ENCRYPTION_KEY_CACHE.write().unwrap();
            *cache = Some(new_key.clone());
        }

        Ok(new_key)
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
        use md5::{Digest, Md5};

        let did = did.unwrap_or(DOUYU_DEFAULT_DID);
        let ts = ts.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });

        // Get encryption key (from cache or fetch new)
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

        Ok(FallbackSignResult {
            auth,
            ts,
            enc_data: enc.enc_data.clone(),
        })
    }

    /// Gets play info using fallback authentication (V1 API)
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

        // Get fallback signature
        let sign_result = self.fallback_sign(rid, Some(did), None).await?;

        // Get the cached key to retrieve user agent
        let key_data = {
            let cache = ENCRYPTION_KEY_CACHE.read().unwrap();
            cache.clone().ok_or_else(|| {
                ExtractorError::ValidationError("No cached encryption key".to_string())
            })?
        };

        let mut form_data: FxHashMap<&str, String> = FxHashMap::default();
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

        // Use V1 API endpoint for fallback auth
        let resp = self
            .extractor
            .client
            .post(format!("https://www.douyu.com/lapi/live/getH5PlayV1/{rid}"))
            .header(reqwest::header::USER_AGENT, &key_data.user_agent)
            .header(reqwest::header::REFERER, Self::BASE_URL)
            .form(&form_data)
            .send()
            .await?
            .json::<DouyuH5PlayResponse>()
            .await?;

        if resp.error != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get play info (fallback): {}",
                resp.msg
            )));
        }

        resp.data.ok_or_else(|| {
            ExtractorError::ValidationError(
                "Failed to get play info (fallback): no data".to_string(),
            )
        })
    }

    /// Clears the encryption key cache
    /// Useful when the key becomes invalid or for testing
    #[allow(dead_code)]
    pub fn clear_encryption_cache() {
        let mut cache = ENCRYPTION_KEY_CACHE.write().unwrap();
        *cache = None;
    }

    // ==================== CDN URL Construction ====================

    /// Parses a Douyu stream URL into its components
    /// Returns the Tencent app name, stream ID, and query parameters
    pub fn parse_stream_url(url: &str) -> Result<ParsedStreamInfo, ExtractorError> {
        // Split URL into base and query string
        let parts: Vec<&str> = url.splitn(2, '?').collect();
        let base_url = parts[0];
        let query_string = parts.get(1).unwrap_or(&"");

        // Parse query parameters
        let mut query_params: HashMap<String, String> = HashMap::new();
        for pair in query_string.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                query_params.insert(
                    key.to_string(),
                    urlencoding::decode(value).unwrap_or_default().to_string(),
                );
            }
        }

        // Extract host from URL
        let host = base_url
            .split("//")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .unwrap_or("")
            .to_string();

        // Extract stream ID (last path segment without extension)
        let stream_id = base_url
            .split('/')
            .next_back()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("")
            .split('_')
            .next()
            .unwrap_or("")
            .to_string();

        // Get Tencent app name from host
        let tx_app_name = Self::get_tx_app_name(&host);

        Ok(ParsedStreamInfo {
            tx_app_name,
            stream_id,
            query_params,
            host,
        })
    }

    /// Gets the Tencent Cloud app name from the RTMP URL host
    /// Maps host suffixes to dyliveflv app names
    fn get_tx_app_name(host: &str) -> String {
        if let Some(captures) = TX_HOST_SUFFIX_REGEX.captures(host)
            && let Some(suffix) = captures.get(1)
        {
            let suffix_str = suffix.as_str();
            // "sa" maps to "1"
            let num = if suffix_str == "sa" { "1" } else { suffix_str };
            return format!("dyliveflv{}", num);
        }
        // Default fallback
        "dyliveflv1".to_string()
    }

    /// Builds a Tencent CDN (tct) URL from stream info
    /// This is used as an intermediate step for building Huoshan URLs
    pub fn build_tencent_url(
        stream_info: &ParsedStreamInfo,
        additional_params: Option<&HashMap<String, String>>,
    ) -> Result<String, ExtractorError> {
        let origin = stream_info
            .query_params
            .get("origin")
            .map(|s| CdnOrigin::from_str(s))
            .unwrap_or(CdnOrigin::Unknown);

        // Validate origin - only tct, hw, dy can be converted to Tencent CDN
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

        // Add additional params if provided
        if let Some(params) = additional_params {
            for (k, v) in params {
                query.insert(k.clone(), v.clone());
            }
        }

        // Remove vhost if present (needed for mobile token)
        query.remove("vhost");

        let query_string = Self::encode_query_params(&query);
        Ok(format!(
            "https://{}/{}/{}.flv?{}",
            tx_host, stream_info.tx_app_name, stream_info.stream_id, query_string
        ))
    }

    /// Builds a Tencent CDN URL using mobile API for looser token validation
    /// This matches the Python implementation's build_tx_url which uses mobile tokens
    async fn build_tencent_url_with_mobile_api(
        &self,
        stream_info: &ParsedStreamInfo,
        token_result: &DouyuTokenResult,
        rid: u64,
    ) -> Result<String, ExtractorError> {
        // Get mobile play info for tct CDN - mobile tokens have looser validation
        let mobile_data = self
            .get_mobile_play_info(
                &token_result.v,
                &token_result.did,
                &token_result.tt,
                &token_result.sign,
                rid,
                "tct-h5",
                self.rate,
            )
            .await?;

        // Parse mobile stream params
        let mobile_params = Self::parse_mobile_stream_params(&mobile_data.url);

        // Build Tencent URL with mobile params
        Self::build_tencent_url_with_mobile_token(stream_info, &mobile_params)
    }

    /// Builds a Huoshan/Volcano CDN (hs-h5) URL
    /// This CDN often provides better performance for some users
    ///
    /// # Arguments
    /// * `stream_info` - Parsed stream info from the original URL
    /// * `tencent_url` - The Tencent CDN URL (used for fp_user_url parameter)
    ///
    /// # Returns
    /// A tuple of (fake_host, cname_url) where:
    /// - fake_host: The Host header value to use when requesting the URL
    /// - cname_url: The actual URL to request
    pub fn build_huoshan_url(
        stream_info: &ParsedStreamInfo,
        tencent_url: &str,
    ) -> Result<(String, String), ExtractorError> {
        // Get the Tencent host from the URL
        let tx_host = tencent_url
            .split("//")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .unwrap_or("tc-tct.douyucdn2.cn");

        // Build Huoshan host from app name
        // dyliveflv1 -> huosa.douyucdn2.cn
        // dyliveflv3 -> huos3.douyucdn2.cn
        let hs_host = stream_info
            .tx_app_name
            .replace("dyliveflv", "huos")
            .replace("huos1", "huosa");
        let hs_host = format!("{}.douyucdn2.cn", hs_host);

        // Build query params for Huoshan URL
        let mut query = stream_info.query_params.clone();
        let encoded_url = urlencoding::encode(tencent_url);
        query.insert("fp_user_url".to_string(), encoded_url.to_string());
        query.insert("vhost".to_string(), tx_host.to_string());
        query.insert("domain".to_string(), tx_host.to_string());

        let query_string = Self::encode_query_params(&query);

        // Huoshan CNAME host
        let hs_cname_host = "douyu-pull.s.volcfcdndvs.com";
        let hs_cname_url = format!(
            "http://{}/live/{}.flv?{}",
            hs_cname_host, stream_info.stream_id, query_string
        );

        Ok((hs_host, hs_cname_url))
    }

    /// Encodes query parameters into a URL query string
    fn encode_query_params(params: &HashMap<String, String>) -> String {
        params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Checks if a CDN type starts with "scdn" (problematic CDN to avoid)
    pub fn is_scdn(cdn: &str) -> bool {
        cdn.starts_with("scdn")
    }

    /// Gets the last available CDN from a list, useful for avoiding scdn
    #[allow(dead_code)]
    pub fn get_fallback_cdn(
        cdns: &[crate::extractor::platforms::douyu::models::CdnsWithName],
    ) -> Option<&str> {
        cdns.last().map(|c| c.cdn.as_str())
    }

    fn create_media_info(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<String>,
        avatar_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
    ) -> MediaInfo {
        MediaInfo::new(
            self.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
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
            return Ok(self.create_media_info("Douyu", "", None, None, false, vec![]));
        }

        let rid = match self.extract_rid(&response) {
            Ok(rid) => rid,
            Err(_) => {
                // If RID extraction fails, try to resolve vanity URL
                debug!("Failed to extract RID from HTML, trying vanity URL resolution");
                self.get_real_room_id(&self.extractor.url).await?
            }
        };

        // Use betard API for more reliable room info including VIP status
        let betard_info = self.get_betard_room_info(rid).await;

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
                    if room.avatar.is_empty() {
                        None
                    } else {
                        Some(room.avatar.clone())
                    },
                )
            }
            Err(e) => {
                // Fallback to HTML parsing and RoomApi
                debug!("Betard API failed, falling back to HTML parsing: {}", e);
                let room_status = self.extract_room_status(&response)?;
                let video_loop = self.extract_video_loop(&response)?;
                let live = room_status == 1 && video_loop == 0;

                // Get room info from RoomApi for metadata
                let room_info = self.get_room_info(rid).await?;
                let room_info = self.parse_room_info(&room_info)?;

                (
                    live,
                    false, // Cannot determine VIP status without betard API
                    room_info.data.room_name.clone(),
                    room_info.data.owner_name.clone(),
                    Some(room_info.data.room_thumb.clone()),
                    Some(room_info.data.avatar.clone()),
                )
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

        // streamer is live - try JS signing first, fallback to server-side auth
        let response_for_thread = Arc::clone(&response);
        let js_token_handle =
            task::spawn_blocking(move || Self::get_js_token(&response_for_thread, rid));

        let js_token_result = js_token_handle.await.unwrap();

        let streams = match js_token_result {
            Ok(js_token) => {
                // JS signing succeeded
                self.get_live_stream_info(&js_token, rid, is_vip).await?
            }
            Err(e) => {
                // JS signing failed, try fallback authentication
                debug!("JS signing failed: {}, trying fallback authentication", e);
                match self.get_streams_with_fallback_auth(rid, is_vip).await {
                    Ok(streams) => streams,
                    Err(fallback_err) => {
                        // Both methods failed, return the original JS error
                        debug!("Fallback authentication also failed: {}", fallback_err);
                        return Err(e);
                    }
                }
            }
        };

        Ok(self.create_media_info(&title, &artist, cover_url, avatar_url, true, streams))
    }

    /// Gets streams using fallback authentication (no JS engine required)
    async fn get_streams_with_fallback_auth(
        &self,
        rid: u64,
        is_vip: bool,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut stream_infos = vec![];

        // Use fallback authentication to get play info
        let data = self
            .get_play_info_fallback(rid, &self.cdn, self.rate, None)
            .await?;

        // Check if we need to build hs-h5 URL
        let needs_hs_build = self.cdn == "hs-h5" && (self.force_hs || data.rtmp_cdn != "hs-h5");

        // Build the base stream URL
        let base_stream_url = format!("{}/{}", data.rtmp_url, data.rtmp_live);

        // If hs-h5 is requested and we need to build it, construct the URL
        let (final_stream_url, hs_host) = if needs_hs_build {
            debug!(
                "Building hs-h5 URL with fallback auth (force_hs={}, rtmp_cdn={})",
                self.force_hs, data.rtmp_cdn
            );

            match Self::parse_stream_url(&base_stream_url) {
                Ok(stream_info) => {
                    // First build Tencent URL if not already tct
                    let is_tct = data.rtmp_cdn == "tct-h5";
                    let tct_url = if is_tct {
                        base_stream_url.clone()
                    } else {
                        match Self::build_tencent_url(&stream_info, None) {
                            Ok(url) => url,
                            Err(e) => {
                                debug!("Failed to build Tencent URL: {}, using original", e);
                                base_stream_url.clone()
                            }
                        }
                    };

                    // Then build Huoshan URL
                    match Self::build_huoshan_url(&stream_info, &tct_url) {
                        Ok((host, url)) => {
                            debug!("Built hs-h5 URL: {} (Host: {})", url, host);
                            (url, Some(host))
                        }
                        Err(e) => {
                            debug!("Failed to build Huoshan URL: {}, using original", e);
                            (base_stream_url.clone(), None)
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to parse stream URL: {}, using original", e);
                    (base_stream_url.clone(), None)
                }
            }
        } else {
            (base_stream_url.clone(), None)
        };

        for cdn in &data.cdns {
            for rate in &data.multirates {
                let stream_url = if cdn.cdn == self.cdn && rate.rate == self.rate as u64 {
                    final_stream_url.clone()
                } else {
                    "".to_string()
                };

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

                let mut extras = serde_json::json!({
                    "cdn": cdn.cdn,
                    "rate": rate.rate,
                    "rid": rid,
                    "is_vip": is_vip,
                    "auth_method": "fallback",
                });

                // Add host header if hs-h5 URL was built
                if let Some(ref host) = hs_host {
                    extras["host_header"] = serde_json::Value::String(host.clone());
                }

                let stream = StreamInfo {
                    url: stream_url,
                    stream_format: format,
                    media_format,
                    quality: rate.name.to_string(),
                    bitrate: rate.bit,
                    priority: 0,
                    extras: Some(extras),
                    codec: codec.to_string(),
                    fps: 0.0,
                    is_headers_needed: hs_host.is_some(),
                };
                stream_infos.push(stream);
            }
        }

        Ok(stream_infos)
    }

    const JS_DOM: &str = "
        encripted = {decryptedCodes: []};
        if (!this.document) {document = {}}
    ";

    const JS_DEBUG: &str = "
        var encripted_fun = ub98484234;
        ub98484234 = function(p1, p2, p3) {
            try {
                encripted.sign = encripted_fun(p1, p2, p3);
            } catch(e) {
                encripted.sign = e.message;
            }
            return encripted;
        }
    ";

    fn get_js_token(response: &str, rid: u64) -> Result<DouyuTokenResult, ExtractorError> {
        let encoded_script = ENCODED_SCRIPT_REGEX
            .captures(response)
            .and_then(|c| c.get(1))
            .map_or("", |m| m.as_str());

        let did = Uuid::new_v4().to_string().replace("-", "");
        // epoch seconds
        let tt = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        // debug!("tt: {}", tt);

        let md5_encoded_script = include_str!("../../../resources/crypto-js-md5.min.js");

        let final_js = format!(
            "{}\n{}\n{}\n{}\nub98484234('{}', '{}', '{}')",
            md5_encoded_script,
            Self::JS_DOM,
            encoded_script,
            Self::JS_DEBUG,
            rid,
            did,
            tt
        );

        // debug!("final_js: {}", final_js);

        let mut context = boa_engine::Context::default();
        let eval_result = context
            .eval(boa_engine::Source::from_bytes(final_js.as_bytes()))
            .map_err(|e| ExtractorError::JsError(e.to_string()))?;
        let res = eval_result.as_object();

        if let Some(res) = res {
            // something like "v=220120250706&did=10000000000000000000000000003306&tt=1751804526&sign=5b1ce0e5888977265b4b378d1b3dcd98"
            let sign = res
                .get(PropertyKey::String("sign".into()), &mut context)
                .map_err(|e| ExtractorError::JsError(e.to_string()))?
                .as_string()
                .map_or("".to_string(), |m| m.to_std_string().unwrap());
            debug!("sign: {}", sign);

            let sign_captures = SIGN_REGEX.captures(&sign);
            if let Some(captures) = sign_captures {
                let v = captures.get(1).unwrap().as_str();
                let did = captures.get(2).unwrap().as_str();
                let tt = captures.get(3).unwrap().as_str();
                let sign = captures.get(4).unwrap().as_str();

                Ok(DouyuTokenResult::new(v, did, tt, sign))
            } else {
                Err(ExtractorError::JsError(
                    "Failed to get js token".to_string(),
                ))
            }
        } else {
            Err(ExtractorError::JsError(
                "Failed to get js token".to_string(),
            ))
        }
    }

    async fn call_get_h5_play(
        &self,
        token_result: &DouyuTokenResult,
        rid: u64,
        cdn: &str,
        rate: i64,
    ) -> Result<DouyuH5PlayData, ExtractorError> {
        let mut form_data: FxHashMap<&str, String> = FxHashMap::default();
        form_data.insert("v", token_result.v.to_string());
        form_data.insert("did", token_result.did.to_string());
        form_data.insert("tt", token_result.tt.to_string());
        form_data.insert("sign", token_result.sign.to_string());
        form_data.insert("cdn", cdn.to_string());
        form_data.insert("rate", rate.to_string());
        form_data.insert("iar", "0".to_string());
        form_data.insert("ive", "0".to_string());

        let resp = self
            .extractor
            .client
            .post(format!(
                "https://playweb.douyucdn.cn/lapi/live/getH5Play/{rid}"
            ))
            .form(&form_data)
            .send()
            .await?
            .json::<DouyuH5PlayResponse>()
            .await?;

        if resp.error != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get live stream info: {}",
                resp.msg
            )));
        }

        resp.data.ok_or_else(|| {
            ExtractorError::ValidationError("Failed to get live stream info: no data".to_string())
        })
    }

    /// Maximum number of retries when avoiding scdn
    const MAX_SCDN_RETRIES: u32 = 2;

    async fn get_live_stream_info(
        &self,
        token_result: &DouyuTokenResult,
        rid: u64,
        is_vip: bool,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut stream_infos = vec![];

        // Try to get play info, with scdn avoidance
        let (data, actual_cdn) = self
            .get_play_info_with_scdn_avoidance(token_result, rid)
            .await?;

        // Check if we need to build hs-h5 URL
        let needs_hs_build = self.cdn == "hs-h5" && (self.force_hs || data.rtmp_cdn != "hs-h5");

        // Build the base stream URL
        let base_stream_url = format!("{}/{}", data.rtmp_url, data.rtmp_live);

        // If hs-h5 is requested and we need to build it, construct the URL
        let (final_stream_url, hs_host) = if needs_hs_build {
            debug!(
                "Building hs-h5 URL (force_hs={}, rtmp_cdn={})",
                self.force_hs, data.rtmp_cdn
            );

            match Self::parse_stream_url(&base_stream_url) {
                Ok(stream_info) => {
                    // First build Tencent URL if not already tct
                    let is_tct = data.rtmp_cdn == "tct-h5";
                    let tct_url = if is_tct {
                        base_stream_url.clone()
                    } else {
                        // Try mobile API first for looser token validation (matches Python)
                        match self
                            .build_tencent_url_with_mobile_api(&stream_info, token_result, rid)
                            .await
                        {
                            Ok(url) => {
                                debug!("Built Tencent URL using mobile API");
                                url
                            }
                            Err(e) => {
                                debug!(
                                    "Mobile API failed: {}, falling back to direct conversion",
                                    e
                                );
                                match Self::build_tencent_url(&stream_info, None) {
                                    Ok(url) => url,
                                    Err(e) => {
                                        debug!(
                                            "Failed to build Tencent URL: {}, using original",
                                            e
                                        );
                                        base_stream_url.clone()
                                    }
                                }
                            }
                        }
                    };

                    // Then build Huoshan URL
                    match Self::build_huoshan_url(&stream_info, &tct_url) {
                        Ok((host, url)) => {
                            debug!("Built hs-h5 URL: {} (Host: {})", url, host);
                            (url, Some(host))
                        }
                        Err(e) => {
                            debug!("Failed to build Huoshan URL: {}, using original", e);
                            (base_stream_url.clone(), None)
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to parse stream URL: {}, using original", e);
                    (base_stream_url.clone(), None)
                }
            }
        } else {
            (base_stream_url.clone(), None)
        };

        for cdn in &data.cdns {
            debug!("cdn: {:?}", cdn);
            for rate in &data.multirates {
                debug!("rate: {:?}", rate);
                // Use configured rate for matching, compute URL only for matching cdn/rate
                let stream_url = if cdn.cdn == actual_cdn && rate.rate == self.rate as u64 {
                    final_stream_url.clone()
                } else {
                    "".to_string()
                };

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

                let mut extras = serde_json::json!({
                    "cdn": cdn.cdn,
                    "rate": rate.rate,
                    "rid": rid,
                    "sign" : token_result.sign,
                    "v" : token_result.v,
                    "did" : token_result.did,
                    "tt" : token_result.tt,
                    "is_vip": is_vip,
                    "actual_cdn": actual_cdn,
                    "auth_method": "js",
                });

                // Add host header if hs-h5 URL was built
                if let Some(ref host) = hs_host {
                    extras["host_header"] = serde_json::Value::String(host.clone());
                }

                let stream = StreamInfo {
                    url: stream_url,
                    stream_format: format,
                    media_format,
                    quality: rate.name.to_string(),
                    bitrate: rate.bit,
                    priority: 0,
                    extras: Some(extras),
                    codec: codec.to_string(),
                    fps: 0.0,
                    is_headers_needed: hs_host.is_some(),
                };
                stream_infos.push(stream);
            }
        }
        Ok(stream_infos)
    }

    /// Gets play info with automatic scdn avoidance
    /// If the returned CDN starts with "scdn", it will retry with a fallback CDN
    async fn get_play_info_with_scdn_avoidance(
        &self,
        token_result: &DouyuTokenResult,
        rid: u64,
    ) -> Result<(DouyuH5PlayData, String), ExtractorError> {
        let mut current_cdn = self.cdn.clone();

        for attempt in 0..Self::MAX_SCDN_RETRIES {
            let resp = self
                .call_get_h5_play(token_result, rid, &current_cdn, self.rate)
                .await?;

            // Check if the returned CDN is scdn (problematic)
            if Self::is_scdn(&resp.rtmp_cdn) {
                debug!(
                    "Attempt {}: Got scdn '{}', trying to avoid",
                    attempt + 1,
                    resp.rtmp_cdn
                );

                // Try to find a fallback CDN from the available list
                if let Some(fallback) = Self::find_non_scdn_fallback(&resp.cdns) {
                    debug!("Switching from scdn to fallback CDN: {}", fallback);
                    current_cdn = fallback;
                    continue;
                } else {
                    // No fallback available, use what we have
                    debug!("No non-scdn fallback available, using scdn");
                    return Ok((resp, current_cdn));
                }
            }

            // Not scdn, we're good
            debug!("Using CDN: {} (rtmp_cdn: {})", current_cdn, resp.rtmp_cdn);
            return Ok((resp, current_cdn));
        }

        // Exhausted retries, try one more time with the current CDN
        let resp = self
            .call_get_h5_play(token_result, rid, &current_cdn, self.rate)
            .await?;
        Ok((resp, current_cdn))
    }

    /// Finds a non-scdn fallback CDN from the available CDN list
    /// Prefers the last CDN in the list (as per Python implementation)
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
            .as_u64()
            .ok_or_else(|| ExtractorError::ValidationError("Missing rid in extras".to_string()))?;

        let tt = extras["tt"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing tt in extras".to_string()))?;
        let v = extras["v"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing v in extras".to_string()))?;
        let did = extras["did"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing did in extras".to_string()))?;
        let sign = extras["sign"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing sign in extras".to_string()))?;

        let token_result = DouyuTokenResult::new(v, did, tt, sign);

        let cdn = extras["cdn"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing cdn in extras".to_string()))?;

        let rate = extras["rate"]
            .as_i64()
            .ok_or_else(|| ExtractorError::ValidationError("Missing rate in extras".to_string()))?;

        let resp = self.call_get_h5_play(&token_result, rid, cdn, rate).await?;

        stream_info.url = format!("{}/{}", resp.rtmp_url, resp.rtmp_live);

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

        let url = "https://www.douyu.com/8440385";

        let extractor = Douyu::new(url.to_string(), default_client(), None, None);
        let media_info = extractor.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
