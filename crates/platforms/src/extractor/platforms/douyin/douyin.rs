use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::douyin::apis::{LIVE_DOUYIN_URL, WEBCAST_ENTER_URL};
use crate::extractor::platforms::douyin::models::{DouyinPcResponse, DouyinQuality};
use crate::extractor::platforms::douyin::utils::{
    GlobalTtwidManager, extract_rid, fetch_ttwid, generate_ms_token, generate_nonce,
    generate_odin_ttid, get_common_params,
};
use crate::media::media_format::MediaFormat;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

pub struct DouyinExtractor {
    extractor: Extractor,
    pub force_origin_quality: bool,
    /// Whether to use the global shared ttwid (default: true) or per-extractor ttwid (false)
    pub use_global_ttwid: bool,
}

impl DouyinExtractor {
    pub fn new(platform_url: String, client: Client) -> Self {
        let mut extractor = Extractor::new("Douyin".to_string(), platform_url.clone(), client);
        let common_params = get_common_params();
        for (key, value) in common_params {
            extractor.add_param(key.to_string(), value.to_string());
        }
        extractor.insert_header(
            reqwest::header::REFERER.to_string(),
            LIVE_DOUYIN_URL.to_string(),
        );
        extractor.insert_header(
            reqwest::header::USER_AGENT.to_string(),
            DEFAULT_UA.to_string(),
        );

        Self {
            extractor,
            force_origin_quality: true,
            use_global_ttwid: true,
        }
    }

    /// Configure whether this extractor should use the global shared ttwid or its own per-extractor ttwid.
    ///
    /// # Arguments
    ///
    /// * `use_global` - true to use global shared ttwid (default), false for per-extractor ttwid
    ///
    /// # Example
    ///
    /// ```rust
    /// let mut extractor = DouyinExtractor::new(url, client);
    /// extractor.set_use_global_ttwid(false); // Use per-extractor ttwid
    /// ```
    pub fn set_use_global_ttwid(&mut self, use_global: bool) {
        self.use_global_ttwid = use_global;
    }

    /// Set msToken cookie specifically (commonly needed for Douyin)
    pub fn set_ms_token(&mut self, ms_token: String) {
        self.extractor.update_param("msToken".to_string(), ms_token);
    }

    /// Set ttwid cookie (another common Douyin cookie)
    /// This sets the per-extractor ttwid and automatically switches to per-extractor mode.
    pub fn set_ttwid(&mut self, ttwid: String) {
        self.extractor.add_cookie("ttwid".to_string(), ttwid);
        // When manually setting a ttwid, switch to per-extractor mode
        self.use_global_ttwid = false;
    }

    /// Set common Douyin cookies all at once
    pub fn set_douyin_cookies(&mut self, ttwid: Option<String>) {
        if let Some(id) = ttwid {
            self.set_ttwid(id); // This will switch to per-extractor mode
        }
    }

    /// Fetch a fresh ttwid and store it based on the current mode (global or per-extractor).
    ///
    /// # Returns
    ///
    /// The ttwid value that was fetched and stored.
    ///
    /// # Example
    ///
    /// ```rust
    /// let mut extractor = DouyinExtractor::new(url, client);
    /// let ttwid = extractor.fetch_and_store_ttwid().await?;
    /// // ttwid is now automatically included in all subsequent requests
    /// ```
    pub async fn fetch_and_store_ttwid(&mut self) -> Result<String, ExtractorError> {
        if self.use_global_ttwid {
            // Use global ttwid management
            let ttwid =
                GlobalTtwidManager::fetch_and_store_global_ttwid(&self.extractor.client).await?;
            // Also store it in this extractor's cookies for automatic inclusion in requests
            self.extractor
                .add_cookie("ttwid".to_string(), ttwid.clone());
            Ok(ttwid)
        } else {
            // Use per-extractor ttwid management (original implementation)
            self.fetch_and_store_per_extractor_ttwid().await
        }
    }

    /// Fetch a fresh ttwid specifically for this extractor instance (per-extractor mode).
    async fn fetch_and_store_per_extractor_ttwid(&mut self) -> Result<String, ExtractorError> {
        // Check if we already have a ttwid cookie
        if let Some(existing_ttwid) = self.extractor.get_cookie("ttwid") {
            debug!("Using existing per-extractor ttwid: {}", existing_ttwid);
            return Ok(existing_ttwid.clone());
        }

        debug!("Fetching fresh ttwid for this extractor (per-extractor mode)");

        let ttwid = fetch_ttwid(&self.extractor.client).await;

        debug!("Fetched per-extractor ttwid: {}", ttwid);

        // Store the ttwid in this extractor's cookie store
        self.extractor
            .add_cookie("ttwid".to_string(), ttwid.clone());

        Ok(ttwid)
    }

    /// Ensure this extractor has a valid ttwid, fetching one if necessary.
    /// Uses either global or per-extractor mode based on configuration.
    ///
    /// # Returns
    ///
    /// The ttwid value (either existing or newly fetched)
    pub async fn ensure_ttwid(&mut self) -> Result<String, ExtractorError> {
        if self.use_global_ttwid {
            // Check if we have the global ttwid in our cookies already
            if let Some(existing_ttwid) = self.extractor.get_cookie("ttwid") {
                return Ok(existing_ttwid.clone());
            }

            // Get or fetch the global ttwid
            let global_ttwid =
                GlobalTtwidManager::ensure_global_ttwid(&self.extractor.client).await?;
            // Store it in our cookies for automatic inclusion
            self.extractor
                .add_cookie("ttwid".to_string(), global_ttwid.clone());
            Ok(global_ttwid)
        } else {
            // Per-extractor mode
            if let Some(existing_ttwid) = self.extractor.get_cookie("ttwid") {
                Ok(existing_ttwid.clone())
            } else {
                self.fetch_and_store_per_extractor_ttwid().await
            }
        }
    }

    /// Get the current ttwid being used by this extractor.
    /// This will return the global ttwid if in global mode, or the per-extractor ttwid if in per-extractor mode.
    pub fn get_current_ttwid(&self) -> Option<String> {
        if self.use_global_ttwid {
            // Return global ttwid if available, otherwise check our cookies
            GlobalTtwidManager::get_global_ttwid()
                .or_else(|| self.extractor.get_cookie("ttwid").cloned())
        } else {
            // Per-extractor mode - only check our cookies
            self.extractor.get_cookie("ttwid").cloned()
        }
    }

    pub async fn get_ms_token(&self) -> Result<String, ExtractorError> {
        let ms_token = self.extractor.get_cookie("msToken").cloned();
        if let Some(token) = ms_token {
            debug!("Using existing msToken: {}", token);
            return Ok(token);
        }
        let ms_token = generate_ms_token();

        Ok(ms_token)
    }

    pub async fn ensure_ms_token(&mut self) -> Result<(), ExtractorError> {
        let ms_token = self.get_ms_token().await?;
        self.set_ms_token(ms_token);
        Ok(())
    }

    pub async fn ensure_odin_ttid(&mut self) -> Result<(), ExtractorError> {
        let odin_ttid = self.extractor.get_param("odin_ttid");
        if odin_ttid.is_some() {
            return Ok(());
        }
        let odin_ttid = generate_odin_ttid();
        self.extractor
            .update_cookie("odin_ttid".to_string(), odin_ttid.clone());
        Ok(())
    }

    pub async fn ensure_nonce(&mut self) -> Result<(), ExtractorError> {
        let nonce = self.extractor.get_param("__ac_nonce");
        if nonce.is_some() {
            return Ok(());
        }
        let nonce = generate_nonce();
        self.extractor
            .update_cookie("__ac_nonce".to_string(), nonce.clone());
        Ok(())
    }

    /// Get access to the base extractor for direct cookie management
    pub fn get_extractor_mut(&mut self) -> &mut Extractor {
        &mut self.extractor
    }

    pub(crate) async fn get_pc_response(&mut self, rid: &str) -> Result<String, ExtractorError> {
        // ensure ttwid is set
        self.ensure_ttwid().await?;
        self.ensure_ms_token().await?;
        self.ensure_odin_ttid().await?;
        self.ensure_nonce().await?;

        let response = self
            .extractor
            .get(WEBCAST_ENTER_URL)
            .query(&[("web_rid", rid)])
            .send()
            .await
            .map_err(|e| ExtractorError::HttpError(e.into()))?;

        // Automatically parse and store new cookies from the response
        self.extractor.parse_and_store_cookies(response.headers());

        let body = response.text().await?;
        // debug!("response body: {}", body);
        Ok(body)
    }

    pub(crate) async fn parse_pc_response(&self, body: &str) -> Result<MediaInfo, ExtractorError> {
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ));
        }

        let response: DouyinPcResponse = serde_json::from_str(body)?;

        // debug!("response: {:?}", response);

        let prompts = response.data.prompts.as_ref();
        if prompts.is_some() {
            return Err(ExtractorError::ValidationError(format!(
                "Error ocurred : {}",
                prompts.unwrap()
            )));
        }

        // Check if data exists first
        // TODO : FALLBACK TO MOBILE API
        if response.data.data.is_empty() {
            return Err(ExtractorError::ValidationError(
                "No room data available".to_string(),
            ));
        }

        let data =
            response.data.data.first().ok_or_else(|| {
                ExtractorError::ValidationError("No room data available".to_string())
            })?;

        let status = data.status;
        let is_live = status == 2;

        let title = data.title.to_string();
        let user = &response.data.user;
        let artist = user.nickname.to_string();
        let avatar_urls: Vec<std::borrow::Cow<'_, str>> = user.avatar_thumb.url_list.clone();
        let is_banned = artist == "账号已注销"
            && avatar_urls
                .iter()
                .any(|url| url.contains("aweme_default_avatar.png"));

        if is_banned {
            return Err(ExtractorError::StreamerNotFound);
        }

        if !is_live {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                title,
                artist,
                None,
                data.cover
                    .as_ref()
                    .and_then(|cover| cover.url_list.first())
                    .map(|url| url.to_string()),
                is_live,
                Vec::new(),
                None,
            ));
        }
        let cover = data
            .cover
            .as_ref()
            .unwrap()
            .url_list
            .first()
            .unwrap()
            .to_string();

        let stream_url = data.stream_url.as_ref().unwrap();

        // Extract stream information
        let mut streams = Vec::new();
        let sdk_pull_data = &stream_url.live_core_sdk_data.pull_data;
        let stream_data = &sdk_pull_data.stream_data;
        let qualities = &sdk_pull_data.options.qualities;

        if self.force_origin_quality {
            // The "ao" (audio-only) quality might be in stream_data but not in the main qualities list.
            debug!("stream data: {:?}", stream_data.data);
            if let Some(ao_quality_data) = stream_data.data.get("ao") {
                if !ao_quality_data.main.flv.is_empty() {
                    // Remove only_audio=1 param to get the video stream
                    let origin_url = ao_quality_data
                        .main
                        .flv
                        .replace("&only_audio=1", "&only_audio=0");

                    // Find the best quality to use for metadata
                    let origin_quality_details = qualities.iter().max_by_key(|q| q.level);

                    let (quality_name, bitrate, codec, extras) = if let Some(details) =
                        origin_quality_details
                    {
                        let mut extras_map = HashMap::new();
                        extras_map.insert("resolution".to_string(), details.resolution.to_string());
                        extras_map.insert("sdk_key".to_string(), details.sdk_key.to_string());
                        let extras = Some(Arc::new(extras_map));
                        (
                            "原画".to_string(),
                            details.v_bit_rate as u32,
                            if details.v_codec == "h264" {
                                "avc".to_string()
                            } else if details.v_codec == "265" {
                                "hevc".to_string()
                            } else {
                                details.v_codec.to_string()
                            },
                            extras,
                        )
                    } else {
                        ("原画".to_string(), 0, "".to_string(), None)
                    };

                    streams.push(StreamInfo {
                        url: origin_url,
                        format: MediaFormat::Flv,
                        quality: quality_name,
                        bitrate,
                        priority: 10, // High priority for origin quality
                        extras,
                        codec,
                        is_headers_needed: false,
                    });
                }
            }
        }

        // debug!("stream data from sdk: {:?}", stream_data.data);

        if !stream_data.data.is_empty() {
            // Build streams from stream_data if available
            let quality_map: HashMap<&str, &DouyinQuality> =
                qualities.iter().map(|q| (q.sdk_key, q)).collect();

            for (sdk_key, quality_data) in &stream_data.data {
                let quality_details = quality_map.get(sdk_key.as_str());

                let (quality_name, bitrate, codec) = if let Some(details) = quality_details {
                    (
                        details.name.to_string(),
                        details.v_bit_rate as u32,
                        if details.v_codec == "264" {
                            "avc".to_string()
                        } else if details.v_codec == "265" {
                            "hevc".to_string()
                        } else {
                            details.v_codec.to_string().clone()
                        },
                    )
                } else {
                    // Fallback if no details found
                    (sdk_key.clone(), 0, "".to_string())
                };

                let mut extras_map = HashMap::new();
                if let Some(details) = quality_details {
                    extras_map.insert("resolution".to_string(), details.resolution.to_string());
                    extras_map.insert("sdk_key".to_string(), details.sdk_key.to_string());
                }
                let extras = if extras_map.is_empty() {
                    None
                } else {
                    Some(Arc::new(extras_map))
                };

                // FLV stream
                if !quality_data.main.flv.is_empty() {
                    streams.push(StreamInfo {
                        url: quality_data.main.flv.clone(),
                        format: MediaFormat::Flv,
                        quality: quality_name.clone(),
                        bitrate,
                        priority: 0,
                        extras: extras.clone(),
                        codec: codec.clone(),
                        is_headers_needed: false,
                    });
                }

                // HLS stream
                if !quality_data.main.hls.is_empty() {
                    streams.push(StreamInfo {
                        url: quality_data.main.hls.clone(),
                        format: MediaFormat::Hls,
                        quality: quality_name.clone(),
                        bitrate,
                        priority: 0,
                        extras: extras.clone(),
                        codec: codec.clone(),
                        is_headers_needed: false,
                    });
                }
            }
        }

        // Fallback to old method if stream_data is empty or no streams were found
        if streams.is_empty() {
            // Add FLV streams
            for (quality, url) in &stream_url.flv_pull_url {
                let stream_info = StreamInfo {
                    url: url.to_string(),
                    format: MediaFormat::Flv,
                    quality: format!("{}", quality),
                    bitrate: 0, // Will be populated from quality data if available
                    priority: 0,
                    extras: None,
                    codec: "".to_string(),
                    is_headers_needed: false,
                };
                streams.push(stream_info);
            }

            // Add HLS streams
            for (quality, url) in &stream_url.hls_pull_url_map {
                let stream_info = StreamInfo {
                    url: url.to_string(),
                    format: MediaFormat::Hls,
                    quality: format!("HLS-{}", quality),
                    bitrate: 0,
                    priority: 0,
                    extras: None,
                    codec: "".to_string(),
                    is_headers_needed: false,
                };
                streams.push(stream_info);
            }
        }

        let media_info = MediaInfo::new(
            self.extractor.url.clone(),
            title,
            artist,
            Some(cover),
            Some(avatar_urls.first().unwrap().to_string()),
            is_live,
            streams,
            None,
        );

        Ok(media_info)
    }
}

#[async_trait]
impl PlatformExtractor for DouyinExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        // Create a mutable copy for cookie management
        let mut extractor_copy = self.clone();
        let web_rid = extract_rid(&self.extractor.url)?;
        debug!("extract web_rid: {}", web_rid);

        let pc_response = extractor_copy.get_pc_response(&web_rid).await?;
        extractor_copy.parse_pc_response(&pc_response).await
    }
}

impl Clone for DouyinExtractor {
    fn clone(&self) -> Self {
        Self {
            extractor: self.extractor.clone(),
            force_origin_quality: self.force_origin_quality,
            use_global_ttwid: self.use_global_ttwid,
        }
    }
}

mod tests {

    use crate::extractor::{
        default::default_client,
        extractor::PlatformExtractor,
        platforms::douyin::{
            DouyinExtractor,
            utils::{DEFAULT_TTWID, GlobalTtwidManager},
        },
    };

    const TEST_URL: &str = "https://live.douyin.com/Shenxin543";

    #[tokio::test]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
        let extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());
        let media_info = extractor.extract().await;
        println!("media_info: {:?}", media_info);
        assert!(media_info.is_ok());
    }

    #[test]
    fn test_douyin_cookie_management() {
        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Test Douyin-specific cookie methods
        extractor.set_ttwid("test_ttwid".to_string());
        assert_eq!(
            extractor.extractor.get_cookie("ttwid"),
            Some(&"test_ttwid".to_string())
        );

        // Test bulk Douyin cookie setting
        extractor.extractor.clear_cookies();
        extractor.set_douyin_cookies(Some("new_ttwid".to_string()));
        assert_eq!(
            extractor.extractor.get_cookie("ttwid"),
            Some(&"new_ttwid".to_string())
        );
    }

    #[test]
    fn test_base_extractor_cookie_functionality() {
        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Test base extractor cookie methods through get_extractor_mut
        let base_extractor = extractor.get_extractor_mut();

        // Test individual cookie setting
        base_extractor.add_cookie("test_cookie".to_string(), "test_value".to_string());
        assert_eq!(
            base_extractor.get_cookie("test_cookie"),
            Some(&"test_value".to_string())
        );

        // Test cookie string parsing
        base_extractor.set_cookies_from_string("cookie1=value1; cookie2=value2; cookie3=value3");
        assert_eq!(
            base_extractor.get_cookie("cookie1"),
            Some(&"value1".to_string())
        );
        assert_eq!(
            base_extractor.get_cookie("cookie2"),
            Some(&"value2".to_string())
        );
        assert_eq!(
            base_extractor.get_cookie("cookie3"),
            Some(&"value3".to_string())
        );

        // Test has_cookie
        assert!(base_extractor.has_cookie("cookie1"));
        assert!(!base_extractor.has_cookie("nonexistent"));

        // Test cookie removal
        let removed = base_extractor.remove_cookie("cookie1");
        assert_eq!(removed, Some("value1".to_string()));
        assert!(!base_extractor.has_cookie("cookie1"));

        // Test cookie clearing
        base_extractor.clear_cookies();
        assert!(base_extractor.get_cookies().is_empty());
    }

    #[test]
    fn test_cookie_integration() {
        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Set some cookies using different methods
        extractor
            .get_extractor_mut()
            .add_cookie("custom_cookie".to_string(), "custom_value".to_string());
        extractor
            .get_extractor_mut()
            .set_cookies_from_string("parsed_cookie=parsed_value");

        // Verify all cookies are present
        assert_eq!(
            extractor.extractor.get_cookie("msToken"),
            Some(&"douyin_token".to_string())
        );
        assert_eq!(
            extractor.extractor.get_cookie("custom_cookie"),
            Some(&"custom_value".to_string())
        );
        assert_eq!(
            extractor.extractor.get_cookie("parsed_cookie"),
            Some(&"parsed_value".to_string())
        );

        // Verify total cookie count
        assert_eq!(extractor.extractor.get_cookies().len(), 3);
    }

    #[tokio::test]
    async fn test_ttwid_management() {
        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Initially should use global ttwid mode
        assert!(extractor.use_global_ttwid);
        assert!(!extractor.extractor.has_cookie("ttwid"));

        // Set a ttwid manually - this should switch to per-extractor mode
        extractor.set_ttwid("manual_ttwid".to_string());
        assert!(!extractor.use_global_ttwid); // Should switch to per-extractor mode
        assert_eq!(
            extractor.extractor.get_cookie("ttwid"),
            Some(&"manual_ttwid".to_string())
        );

        // ensure_ttwid should return the existing one without making a request
        let ttwid = extractor.ensure_ttwid().await.unwrap();
        assert_eq!(ttwid, "manual_ttwid");

        // Test switching back to global mode
        extractor.set_use_global_ttwid(true);
        assert!(extractor.use_global_ttwid);

        // Clear cookies and test get_current_ttwid behavior
        extractor.extractor.clear_cookies();
        let current_ttwid = extractor.get_current_ttwid();
        // In global mode, it should check global ttwid first

        // Note: We can't easily test the actual network request in unit tests
        // but we can test the logic and fallback behavior
    }

    #[test]
    fn test_global_vs_per_extractor_ttwid() {
        // Clear any existing global ttwid
        GlobalTtwidManager::clear_global_ttwid();

        let mut extractor1 = DouyinExtractor::new(TEST_URL.to_string(), default_client());
        let extractor2 = DouyinExtractor::new(
            "https://live.douyin.com/789012".to_string(),
            default_client(),
        );

        // Both should start in global mode
        assert!(extractor1.use_global_ttwid);
        assert!(extractor2.use_global_ttwid);

        // Set a global ttwid
        GlobalTtwidManager::set_global_ttwid("global_ttwid".to_string());

        // Both extractors should see the same global ttwid
        assert_eq!(
            extractor1.get_current_ttwid(),
            Some("global_ttwid".to_string())
        );
        assert_eq!(
            extractor2.get_current_ttwid(),
            Some("global_ttwid".to_string())
        );

        // Switch extractor1 to per-extractor mode and set a different ttwid
        extractor1.set_ttwid("extractor1_ttwid".to_string());
        assert!(!extractor1.use_global_ttwid); // Should auto-switch to per-extractor mode

        // Now they should have different ttwids
        assert_eq!(
            extractor1.get_current_ttwid(),
            Some("extractor1_ttwid".to_string())
        );
        assert_eq!(
            extractor2.get_current_ttwid(),
            Some("global_ttwid".to_string())
        );

        // extractor2 should still be in global mode
        assert!(extractor2.use_global_ttwid);
    }

    #[test]
    fn test_ttwid_thread_safety() {
        // Test that multiple extractor instances have isolated cookie stores
        // but can share global ttwid when in global mode
        GlobalTtwidManager::clear_global_ttwid();

        let mut extractor1 = DouyinExtractor::new(TEST_URL.to_string(), default_client());
        let mut extractor2 = DouyinExtractor::new(
            "https://live.douyin.com/789012".to_string(),
            default_client(),
        );

        // Set different modes
        extractor1.set_use_global_ttwid(false); // Per-extractor mode
        extractor2.set_use_global_ttwid(true); // Global mode

        // Set different ttwids
        extractor1.set_ttwid("ttwid_1".to_string());
        GlobalTtwidManager::set_global_ttwid("global_ttwid".to_string());

        // Verify they have different ttwids based on their modes
        assert_eq!(extractor1.get_current_ttwid(), Some("ttwid_1".to_string()));
        assert_eq!(
            extractor2.get_current_ttwid(),
            Some("global_ttwid".to_string())
        );

        // Change global ttwid - only extractor2 should be affected
        GlobalTtwidManager::set_global_ttwid("new_global_ttwid".to_string());
        assert_eq!(extractor1.get_current_ttwid(), Some("ttwid_1".to_string())); // Unchanged
        assert_eq!(
            extractor2.get_current_ttwid(),
            Some("new_global_ttwid".to_string())
        ); // Changed
    }

    #[test]
    fn test_mode_switching() {
        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Should start in global mode
        assert!(extractor.use_global_ttwid);

        // Manually switch to per-extractor mode
        extractor.set_use_global_ttwid(false);
        assert!(!extractor.use_global_ttwid);

        // Setting a ttwid should automatically switch to per-extractor mode
        extractor.set_use_global_ttwid(true); // Reset to global
        assert!(extractor.use_global_ttwid);

        extractor.set_ttwid("manual_ttwid".to_string());
        assert!(!extractor.use_global_ttwid); // Should auto-switch to per-extractor mode
    }

    #[test]
    fn test_default_ttwid_fallback() {
        // Test that the DEFAULT_TTWID constant is accessible and valid
        assert!(!DEFAULT_TTWID.is_empty());
        assert!(DEFAULT_TTWID.contains("%7C")); // URL-encoded format

        let mut extractor = DouyinExtractor::new(TEST_URL.to_string(), default_client());

        // Test manual fallback to default
        extractor.set_ttwid(DEFAULT_TTWID.to_string());
        assert_eq!(
            extractor.extractor.get_cookie("ttwid"),
            Some(&DEFAULT_TTWID.to_string())
        );
        assert!(!extractor.use_global_ttwid); // Should be in per-extractor mode after setting
    }
}
