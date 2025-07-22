use crate::extractor::default::DEFAULT_MOBILE_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::douyin::apis::{
    APP_REFLOW_URL, BASE_URL, LIVE_DOUYIN_URL, WEBCAST_ENTER_URL,
};
use crate::extractor::platforms::douyin::models::{
    DouyinAppResponse, DouyinPcData, DouyinPcResponse, DouyinQuality, DouyinStreamDataParsed,
    DouyinStreamExtras, DouyinStreamUrl, DouyinUserInfo,
};
use crate::extractor::platforms::douyin::utils::{
    GlobalTtwidManager, extract_rid, fetch_ttwid, generate_ms_token, generate_nonce,
    generate_odin_ttid, get_common_params,
};
use crate::media::formats::{MediaFormat, StreamFormat};
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, RequestBuilder};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};
use tracing::debug;

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?live.douyin\.com/([a-zA-Z0-9_\-\.]+)").unwrap()
});

/// Manages the ttwid cookie strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtwidManagementMode {
    /// Use a single, globally shared ttwid across all extractors.
    Global,
    /// Use a separate ttwid for each extractor instance.
    PerExtractor,
}

/// Configuration for the Douyin extractor.
///
/// This struct holds immutable configuration settings.
/// Use `DouyinExtractorBuilder` to construct an instance.
pub struct DouyinExtractorConfig {
    /// The base extractor, holding common configuration like URL, client, headers, and params.
    pub extractor: Extractor,
    /// Force the use of origin quality stream if available.
    pub force_origin_quality: bool,
    /// The management mode for the `ttwid` cookie.
    pub ttwid_management_mode: TtwidManagementMode,
    /// A specific `ttwid` to use when in `PerExtractor` mode.
    pub ttwid: Option<String>,
}

/// Builder for `DouyinExtractorConfig`.
pub struct Douyin {
    extractor: Extractor,
    force_origin_quality: bool,
    ttwid_management_mode: TtwidManagementMode,
    ttwid: Option<String>,
}

impl Douyin {
    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Douyin".to_string(), url, client);

        extractor.add_header(
            reqwest::header::REFERER.to_string(),
            LIVE_DOUYIN_URL.to_string(),
        );

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        let common_params = get_common_params();
        for (key, value) in common_params {
            extractor.add_param(key.to_string(), value.to_string());
        }

        let force_origin_quality = extras
            .as_ref()
            .and_then(|extras| extras.get("force_origin_quality").and_then(|v| v.as_bool()))
            .unwrap_or(true);

        let ttwid_management_mode_str = extras
            .as_ref()
            .and_then(|extras| extras.get("ttwid_management_mode").and_then(|v| v.as_str()))
            .map(|v| v.to_string())
            .unwrap_or("global".to_string());

        let ttwid_management_mode = if ttwid_management_mode_str == "global" {
            TtwidManagementMode::Global
        } else {
            TtwidManagementMode::PerExtractor
        };

        let ttwid = extras
            .as_ref()
            .and_then(|extras| extras.get("ttwid").and_then(|v| v.as_str()))
            .map(|v| v.to_string());

        Self {
            extractor,
            force_origin_quality,
            ttwid_management_mode,
            ttwid,
        }
    }

    pub fn force_origin_quality(mut self, force: bool) -> Self {
        self.force_origin_quality = force;
        self
    }

    pub fn ttwid_mode(mut self, mode: TtwidManagementMode) -> Self {
        self.ttwid_management_mode = mode;
        self
    }

    /// Set a specific ttwid to use.
    /// This automatically switches to `PerExtractor` mode.
    pub fn ttwid(mut self, ttwid: String) -> Self {
        self.ttwid = Some(ttwid);
        self.ttwid_management_mode = TtwidManagementMode::PerExtractor;
        self
    }
}

/// Handles the state and logic for a single `extract` call.
struct DouyinRequest<'a> {
    config: &'a Douyin,
    web_rid: String,
    cookies: FxHashMap<String, String>,
    params: FxHashMap<String, String>,
    id_str: Option<String>,
    sec_rid: Option<String>,
}

impl<'a> DouyinRequest<'a> {
    /// Creates a new request handler.
    fn new(cookies: FxHashMap<String, String>, config: &'a Douyin, web_rid: String) -> Self {
        Self {
            config,
            web_rid,
            cookies,
            params: config.extractor.platform_params.clone(),
            id_str: None,
            sec_rid: None,
        }
    }

    /// The main entry point for the extraction logic.
    async fn extract(&mut self) -> Result<MediaInfo, ExtractorError> {
        self.ensure_all_requirements().await?;

        let pc_response = self.get_pc_response().await?;
        match self.parse_pc_response(&pc_response) {
            Ok(media_info) => Ok(media_info),
            Err(err) => {
                if matches!(err, ExtractorError::ValidationError(ref msg) if msg == "No room data available")
                {
                    let app_response = self.get_app_response().await?;
                    return self.parse_app_response(&app_response);
                }
                Err(err)
            }
        }
    }

    /// Creates a `RequestBuilder` with all necessary headers, params, and cookies.
    fn request(&self, method: reqwest::Method, url: &str) -> RequestBuilder {
        let mut cookies = String::new();

        if !self.cookies.is_empty() {
            let cookie_string = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(cookie_value) = reqwest::header::HeaderValue::from_str(&cookie_string) {
                cookies = cookie_value.to_str().unwrap_or("").to_string();
            }
        }

        let mut builder = self.config.extractor.request(method, url);
        if !cookies.is_empty() {
            builder = builder.header(reqwest::header::COOKIE, cookies);
        }
        // debug!("builder: {:?}", builder);
        builder
    }

    /// Fetches the main PC API response.
    async fn get_pc_response(&mut self) -> Result<String, ExtractorError> {
        let response = self
            .request(reqwest::Method::GET, WEBCAST_ENTER_URL)
            .query(&[("web_rid", &self.web_rid)])
            .send()
            .await
            .map_err(ExtractorError::HttpError)?;

        // Automatically parse and store new cookies from the response
        self.parse_and_store_cookies(response.headers());

        response.text().await.map_err(ExtractorError::from)
    }
    async fn get_app_response(&mut self) -> Result<String, ExtractorError> {
        let default_room_id = "2";
        let room_id = self.id_str.as_ref().map_or(default_room_id, |v| v.as_str());
        let sec_rid = self.sec_rid.as_ref().unwrap().as_str();
        let mut builder = self
            .config
            .extractor
            .client
            .request(reqwest::Method::GET, APP_REFLOW_URL)
            .header(reqwest::header::REFERER, BASE_URL)
            .header(reqwest::header::ORIGIN, BASE_URL)
            .header(reqwest::header::USER_AGENT, DEFAULT_MOBILE_UA)
            .query(&[
                ("room_id", room_id),
                ("sec_user_id", sec_rid),
                ("type_id", "0"),
                ("live_id", "1"),
                ("version_code", "99.99.99"),
                ("app_id", "1128"),
                (
                    "msToken",
                    self.params.get("msToken").unwrap_or(&"".to_string()),
                ),
                ("compress", "gzip"),
                ("aid", "6383"),
            ]);

        if !self.cookies.is_empty() {
            let cookie_string = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            builder = builder.header(reqwest::header::COOKIE, cookie_string);
        }
        debug!("builder: {:?}", builder);
        let response = builder.send().await.map_err(ExtractorError::HttpError)?;

        // Automatically parse and store new cookies from the response
        self.parse_and_store_cookies(response.headers());

        response.text().await.map_err(ExtractorError::from)
    }

    /// Parses and stores cookies from response headers.
    fn parse_and_store_cookies(&mut self, headers: &reqwest::header::HeaderMap) {
        for value in headers.get_all("set-cookie").iter() {
            if let Ok(cookie_str) = value.to_str() {
                if let Some(cookie_part) = cookie_str.split(';').next() {
                    if let Some((name, value)) = cookie_part.split_once('=') {
                        self.cookies
                            .insert(name.trim().to_string(), value.trim().to_string());
                    }
                }
            }
        }
    }

    /// Ensures all required parameters and cookies are set before making a request.
    async fn ensure_all_requirements(&mut self) -> Result<(), ExtractorError> {
        self.ensure_ttwid().await?;
        self.ensure_ms_token().await?;
        self.ensure_odin_ttid();
        self.ensure_nonce();
        Ok(())
    }

    /// Ensures a valid `ttwid` cookie is present.
    async fn ensure_ttwid(&mut self) -> Result<(), ExtractorError> {
        if self.cookies.contains_key("ttwid") {
            return Ok(());
        }

        let ttwid = match self.config.ttwid_management_mode {
            TtwidManagementMode::Global => {
                GlobalTtwidManager::ensure_global_ttwid(&self.config.extractor.client).await?
            }
            TtwidManagementMode::PerExtractor => {
                if let Some(ttwid) = &self.config.ttwid {
                    ttwid.clone()
                } else {
                    fetch_ttwid(&self.config.extractor.client).await
                }
            }
        };
        self.cookies.insert("ttwid".to_string(), ttwid);
        Ok(())
    }

    /// Ensures a valid `msToken` is present.
    async fn ensure_ms_token(&mut self) -> Result<(), ExtractorError> {
        if self.params.contains_key("msToken") {
            return Ok(());
        }
        let ms_token = generate_ms_token();
        self.params.insert("msToken".to_string(), ms_token);
        Ok(())
    }

    /// Ensures a valid `odin_ttid` is present.
    fn ensure_odin_ttid(&mut self) {
        if self.params.contains_key("odin_ttid") {
            return;
        }
        let odin_ttid = generate_odin_ttid();
        self.cookies.insert("odin_ttid".to_string(), odin_ttid);
    }

    /// Ensures a valid `__ac_nonce` is present.
    fn ensure_nonce(&mut self) {
        if self.params.contains_key("__ac_nonce") {
            return;
        }
        let nonce = generate_nonce();
        self.cookies.insert("__ac_nonce".to_string(), nonce);
    }
    /// Parses the API response body into `MediaInfo`.
    fn parse_pc_response(&mut self, body: &str) -> Result<MediaInfo, ExtractorError> {
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ));
        }

        let response: DouyinPcResponse = serde_json::from_str(body)?;
        self._validate_response(&response)?;

        let user = response.data.user.as_ref().unwrap();
        self.id_str = response.data.enter_room_id.map(|v| v.to_string());
        self.sec_rid = Some(user.sec_uid.to_string());

        let data = self._extract_data(&response)?;

        self._build_media_info(data, user)
    }

    fn parse_app_response(&mut self, body: &str) -> Result<MediaInfo, ExtractorError> {
        let response: DouyinAppResponse = serde_json::from_str(body)?;
        if let Some(prompts) = &response.data.prompts {
            return Err(ExtractorError::ValidationError(format!(
                "API error: {prompts}",
            )));
        }

        if let Some(user) = &response.data.user {
            if self.is_account_banned(user) {
                return Err(ExtractorError::StreamerBanned);
            }
        } else {
            let msg = response
                .data
                .message
                .as_ref()
                .unwrap_or(&Cow::Borrowed("Unknown error"));
            return Err(ExtractorError::ValidationError(format!("API error: {msg}")));
        }

        let data = &response.data.room;
        let user = response.data.user.as_ref().unwrap();

        self._build_media_info(
            data.as_ref().ok_or_else(|| {
                ExtractorError::ValidationError("Room data not available".to_string())
            })?,
            user,
        )
    }

    /// Validates the overall API response.
    fn _validate_response(&self, response: &DouyinPcResponse) -> Result<(), ExtractorError> {
        if let Some(prompts) = &response.data.prompts {
            return Err(ExtractorError::ValidationError(format!(
                "API error: {prompts}"
            )));
        }
        if let Some(user) = &response.data.user {
            if self.is_account_banned(user) {
                return Err(ExtractorError::StreamerBanned);
            }
        } else {
            let msg = response.data.message.as_ref().unwrap_or(&"Unknown error");
            return Err(ExtractorError::ValidationError(format!("API error: {msg}")));
        }
        Ok(())
    }

    /// Extracts the core room data from the response.
    fn _extract_data<'b>(
        &self,
        response: &'b DouyinPcResponse,
    ) -> Result<&'b DouyinPcData<'b>, ExtractorError> {
        response
            .data
            .data
            .as_ref()
            .and_then(|data| data.first())
            .ok_or_else(|| ExtractorError::ValidationError("No room data available".to_string()))
    }

    /// Builds the final `MediaInfo` struct from the parsed data.
    fn _build_media_info(
        &self,
        data: &DouyinPcData,
        user: &DouyinUserInfo,
    ) -> Result<MediaInfo, ExtractorError> {
        let is_live = data.status == 2;

        let title = &data.title;
        let artist = &user.nickname;
        let cover_url = data
            .cover
            .as_ref()
            .and_then(|cover| cover.url_list.first())
            .map(|url| url.to_string());
        let avatar_url = user
            .avatar_thumb
            .url_list
            .first()
            .map(|url| url.to_string());

        if !is_live {
            return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
        }

        let stream_url = data
            .stream_url
            .as_ref()
            .ok_or_else(|| ExtractorError::ValidationError("Stream is not live".to_string()))?;
        let streams = self.extract_streams(stream_url)?;

        Ok(MediaInfo::new(
            self.config.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.config.extractor.get_platform_headers_map()),
        ))
    }

    fn create_offline_media_info(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<String>,
        avatar_url: Option<String>,
    ) -> MediaInfo {
        MediaInfo::new(
            self.config.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            false,
            Vec::new(),
            None,
        )
    }

    fn is_account_banned(&self, user: &DouyinUserInfo) -> bool {
        user.nickname == "账号已注销"
            && user
                .avatar_thumb
                .url_list
                .iter()
                .any(|url| url.contains("aweme_default_avatar.png"))
    }

    /// Orchestrates the extraction of all available streams.
    fn extract_streams(
        &self,
        stream_url: &DouyinStreamUrl,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let sdk_pull_data = &stream_url.live_core_sdk_data.pull_data;
        let stream_data = &sdk_pull_data.stream_data;
        let qualities = &sdk_pull_data.options.qualities;

        let mut streams = Vec::new();

        // 1. Attempt to extract origin quality stream if forced
        let mut origin_quality_filled = false;
        if self.config.force_origin_quality {
            if let Some(origin_stream) = self._extract_origin_stream(stream_data, qualities) {
                streams.push(origin_stream);
                origin_quality_filled = true;
            }
        }

        // 2. Extract streams from SDK data
        if !stream_data.data.is_empty() {
            let sdk_streams =
                self._extract_sdk_streams(stream_data, qualities, origin_quality_filled);
            streams.extend(sdk_streams);
        }

        // 3. Fallback to legacy stream URLs if no other streams were found
        if streams.is_empty() {
            let legacy_streams = self._extract_legacy_streams(stream_url);
            streams.extend(legacy_streams);
        }

        Ok(streams)
    }

    /// Extracts the "origin" quality stream if available.
    fn _extract_origin_stream(
        &self,
        stream_data: &DouyinStreamDataParsed,
        qualities: &[DouyinQuality],
    ) -> Option<StreamInfo> {
        let ao_quality_data = stream_data.data.get("ao")?;
        if ao_quality_data.main.flv.is_empty() {
            return None;
        }

        let origin_url = ao_quality_data
            .main
            .flv
            .replace("&only_audio=1", "&only_audio=0");
        let origin_quality_details = qualities.iter().find(|q| q.sdk_key == "origin");

        let (quality_name, bitrate, codec, fps, extras) = match origin_quality_details {
            Some(details) => (
                "原画",
                Self::normalize_bitrate(details.v_bit_rate.try_into().unwrap()),
                Self::normalize_codec(details.v_codec),
                details.fps,
                Some(Arc::new(DouyinStreamExtras {
                    resolution: details.resolution.to_string(),
                    sdk_key: details.sdk_key.to_string(),
                })),
            ),
            None => ("原画", 0, String::new(), 0, None),
        };

        Some(StreamInfo {
            url: origin_url,
            stream_format: StreamFormat::Flv,
            media_format: MediaFormat::Flv,
            quality: quality_name.to_string(),
            bitrate: bitrate as u64,
            priority: 10,
            extras: extras.map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null)),
            codec,
            fps: fps as f64,
            is_headers_needed: false,
        })
    }

    /// Extracts all available streams from the SDK data.
    fn _extract_sdk_streams(
        &self,
        stream_data: &DouyinStreamDataParsed,
        qualities: &[DouyinQuality],
        origin_quality_filled: bool,
    ) -> Vec<StreamInfo> {
        let mut streams = Vec::new();
        let quality_map: FxHashMap<&str, &DouyinQuality> =
            qualities.iter().map(|q| (q.sdk_key, q)).collect();

        for (sdk_key, quality_data) in stream_data.data.iter() {
            if origin_quality_filled && sdk_key == "origin" {
                continue;
            }

            let quality_details = quality_map.get(sdk_key.as_str());
            let (quality_name, bitrate, fps, codec) = match quality_details {
                Some(details) => (
                    details.name,
                    Self::normalize_bitrate(details.v_bit_rate as u32),
                    details.fps,
                    Self::normalize_codec(details.v_codec),
                ),
                None => (sdk_key.as_str(), 0, 0, String::new()),
            };

            let extras = quality_details.map(|details| {
                Arc::new(DouyinStreamExtras {
                    resolution: details.resolution.to_string(),
                    sdk_key: details.sdk_key.to_string(),
                })
            });

            self._add_stream_if_url_present(
                &quality_data.main.flv,
                StreamFormat::Flv,
                MediaFormat::Flv,
                quality_name,
                bitrate as u64,
                &codec,
                fps,
                extras.as_ref(),
                &mut streams,
            );

            self._add_stream_if_url_present(
                &quality_data.main.hls,
                StreamFormat::Hls,
                MediaFormat::Ts,
                quality_name,
                bitrate as u64,
                &codec,
                fps,
                extras.as_ref(),
                &mut streams,
            );
        }
        streams
    }

    /// Helper to add a new `StreamInfo` to the list if the URL is not empty.
    #[allow(clippy::too_many_arguments)]
    fn _add_stream_if_url_present(
        &self,
        url: &str,
        format: StreamFormat,
        media_format: MediaFormat,
        quality_name: &str,
        bitrate: u64,
        codec: &str,
        fps: i32,
        extras: Option<&Arc<DouyinStreamExtras>>,
        streams: &mut Vec<StreamInfo>,
    ) {
        if !url.is_empty() {
            streams.push(StreamInfo {
                url: url.to_string(),
                stream_format: format,
                media_format,
                quality: quality_name.to_string(),
                bitrate,
                priority: 0,
                extras: extras.map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null)),
                codec: codec.to_string(),
                fps: fps as f64,
                is_headers_needed: false,
            });
        }
    }

    /// Extracts streams from the legacy pull URL fields.
    fn _extract_legacy_streams(&self, stream_url: &DouyinStreamUrl) -> Vec<StreamInfo> {
        let mut streams =
            Vec::with_capacity(stream_url.flv_pull_url.len() + stream_url.hls_pull_url_map.len());

        for (quality, url) in &stream_url.flv_pull_url {
            streams.push(StreamInfo {
                url: url.to_string(),
                stream_format: StreamFormat::Flv,
                media_format: MediaFormat::Flv,
                quality: quality.to_string(),
                bitrate: 0,
                priority: 0,
                extras: None,
                fps: 0.0,
                codec: String::new(),
                is_headers_needed: false,
            });
        }

        for (quality, url) in &stream_url.hls_pull_url_map {
            streams.push(StreamInfo {
                url: url.to_string(),
                stream_format: StreamFormat::Hls,
                media_format: MediaFormat::Ts,
                quality: quality.to_string(),
                bitrate: 0,
                priority: 0,
                extras: None,
                fps: 0.0,
                codec: String::new(),
                is_headers_needed: false,
            });
        }
        streams
    }

    fn normalize_codec(codec: &str) -> String {
        match codec {
            "264" => "avc".to_string(),
            "265" => "hevc".to_string(),
            _ => codec.to_string(),
        }
    }

    fn normalize_bitrate(bitrate: u32) -> u32 {
        match bitrate {
            0 => 0,
            _ => bitrate / 1000,
        }
    }
}

#[async_trait]
impl PlatformExtractor for Douyin {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let web_rid = extract_rid(&self.extractor.url)?;
        debug!("extract web_rid: {}", web_rid);

        let mut request = DouyinRequest::new(self.extractor.cookies.clone(), self, web_rid);
        request.extract().await
    }
}

#[cfg(test)]
mod tests {
    use crate::extractor::default::default_client;
    use crate::extractor::platform_extractor::PlatformExtractor;
    use crate::extractor::platforms::douyin::builder::{
        Douyin, DouyinRequest, TtwidManagementMode,
    };
    use crate::extractor::platforms::douyin::models::{DouyinAvatarThumb, DouyinUserInfo};
    use crate::extractor::platforms::douyin::utils::GlobalTtwidManager;

    const TEST_URL: &str = "https://live.douyin.com/Esdeathkami.";

    #[tokio::test]
    #[ignore]
    async fn test_extract_live() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();

        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let media_info = config.extract().await;

        println!("{media_info:?}");
    }

    #[test]
    fn test_builder_defaults() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);

        assert_eq!(config.extractor.url, TEST_URL);
        assert!(config.force_origin_quality);
        assert_eq!(config.ttwid_management_mode, TtwidManagementMode::Global);
        assert!(config.ttwid.is_none());
    }

    #[test]
    fn test_builder_custom_options() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None)
            .force_origin_quality(false)
            .ttwid_mode(TtwidManagementMode::PerExtractor);

        assert!(!config.force_origin_quality);
        assert_eq!(
            config.ttwid_management_mode,
            TtwidManagementMode::PerExtractor
        );
    }

    #[test]
    fn test_builder_with_ttwid() {
        let ttwid = "test_ttwid_123".to_string();
        let config =
            Douyin::new(TEST_URL.to_string(), default_client(), None, None).ttwid(ttwid.clone());

        assert_eq!(config.ttwid, Some(ttwid));
        assert_eq!(
            config.ttwid_management_mode,
            TtwidManagementMode::PerExtractor
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_request_cookie_logic() {
        // 1. Test Global Mode
        GlobalTtwidManager::set_global_ttwid("global_ttwid_for_test");
        let config_global = Douyin::new(TEST_URL.to_string(), default_client(), None, None)
            .ttwid_mode(TtwidManagementMode::Global);

        let mut request_global = DouyinRequest::new(
            config_global.extractor.cookies.clone(),
            &config_global,
            "123".to_string(),
        );
        request_global.ensure_ttwid().await.unwrap();
        assert_eq!(
            request_global.cookies.get("ttwid"),
            Some(&"global_ttwid_for_test".to_string())
        );

        // 2. Test PerExtractor Mode with pre-set ttwid
        let config_per_extractor_set =
            Douyin::new(TEST_URL.to_string(), default_client(), None, None)
                .ttwid("preset_ttwid".to_string());

        let mut request_per_extractor_set = DouyinRequest::new(
            config_per_extractor_set.extractor.cookies.clone(),
            &config_per_extractor_set,
            "123".to_string(),
        );
        request_per_extractor_set.ensure_ttwid().await.unwrap();
        assert_eq!(
            request_per_extractor_set.cookies.get("ttwid"),
            Some(&"preset_ttwid".to_string())
        );

        // 3. Test PerExtractor Mode without pre-set ttwid (will fetch)
        // This part is hard to test without mocking network calls, but we can check the logic path.
        // We know it will call `fetch_ttwid` internally.
    }

    #[test]
    fn test_is_account_banned() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let request =
            DouyinRequest::new(config.extractor.cookies.clone(), &config, "123".to_string());

        let banned_user = DouyinUserInfo {
            id_str: "1",
            sec_uid: "1",
            nickname: "账号已注销",
            avatar_thumb: DouyinAvatarThumb {
                url_list: vec!["http://example.com/aweme_default_avatar.png".into()],
            },
        };
        assert!(request.is_account_banned(&banned_user));

        let active_user = DouyinUserInfo {
            id_str: "2",
            sec_uid: "2",
            nickname: "ActiveUser",
            avatar_thumb: DouyinAvatarThumb {
                url_list: vec!["http://example.com/real_avatar.png".into()],
            },
        };
        assert!(!request.is_account_banned(&active_user));
    }
}
