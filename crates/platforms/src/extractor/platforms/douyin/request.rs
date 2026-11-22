//! Per-extraction HTTP request state, cookie/signature setup, and PC/App response parsing
//! for the Douyin extractor.

use std::borrow::Cow;
use std::collections::HashMap;

use reqwest::RequestBuilder;
use rustc_hash::FxHashMap;
use tracing::debug;

use crate::extractor::default::{DEFAULT_MOBILE_UA, DEFAULT_UA};
use crate::extractor::error::ExtractorError;
use crate::media::media_info::MediaInfo;

use super::abogus::ABogus;
use super::apis::{APP_REFLOW_URL, LIVE_DOUYIN_URL, WEBCAST_ENTER_URL};
use super::builder::{Douyin, TtwidManagementMode};
use super::models::{
    DouyinAppResponse, DouyinAvatarThumb, DouyinPcData, DouyinPcResponse, DouyinUserInfo,
};
use super::sign::gen_verify_fp;
use super::streams::extract_streams;
use super::utils::{GlobalTtwidManager, fetch_ttwid, generate_odin_ttid, get_common_params};

/// Handles the state and logic for a single `extract` call.
pub(super) struct DouyinRequest<'a> {
    pub(super) config: &'a Douyin,
    pub(super) web_rid: String,
    pub(super) cookies: FxHashMap<String, String>,
    pub(super) id_str: Option<String>,
    pub(super) sec_rid: Option<String>,
    pub(super) platform_metadata: Option<crate::media::PlatformMetadata>,
}

impl<'a> DouyinRequest<'a> {
    /// Creates a new request handler.
    pub(super) fn new(
        cookies: FxHashMap<String, String>,
        config: &'a Douyin,
        web_rid: String,
    ) -> Self {
        let mut cookies = cookies;
        cookies.insert("hevc_supported".to_string(), "true".to_string());

        Self {
            config,
            web_rid,
            cookies,
            id_str: None,
            sec_rid: None,
            platform_metadata: None,
        }
    }

    /// The main entry point for the extraction logic.
    pub(super) async fn extract(&mut self) -> Result<MediaInfo, ExtractorError> {
        self.ensure_all_requirements().await?;

        // Force mobile API if configured; PC API is still queried first for sec_user_id and room_id.
        if self.config.force_mobile_api {
            let pc_response = self.get_pc_response().await?;
            if let Err(e) = self.extract_user_info_from_pc_response(&pc_response) {
                debug!(
                    category = e.category(),
                    "Failed to extract user info from Douyin PC response"
                );
                return Err(e);
            }
            let app_response = self.get_app_response().await?;
            return self.parse_app_response(&app_response);
        }

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

    /// Extracts only the user info (`sec_uid` and `room_id`) from PC response.
    pub(super) fn extract_user_info_from_pc_response(
        &mut self,
        body: &str,
    ) -> Result<(), ExtractorError> {
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty response from API".to_string(),
            ));
        }

        let trimmed = body.trim_start();
        if trimmed.starts_with("<!") || trimmed.starts_with("<html") {
            return Err(ExtractorError::ValidationError(
                "Received HTML error page instead of JSON".to_string(),
            ));
        }

        let response: DouyinPcResponse = serde_json::from_str(body)?;

        if let Some(user) = &response.data.user {
            if user.is_cancelled() {
                return Err(ExtractorError::StreamerNotFound);
            }
            self.sec_rid = Some(user.sec_uid.to_string());
            if response.is_success() {
                self.platform_metadata = super::metadata::metadata(&self.web_rid, user.id_str);
            }
        } else {
            let msg = response.data.message.as_ref().unwrap_or(&"Unknown error");
            return Err(ExtractorError::ValidationError(format!("API error: {msg}")));
        }

        self.id_str = response.data.enter_room_id.map(|v| v.to_string());

        Ok(())
    }

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
        if !cookies.is_empty()
            && let Ok(mut val) = reqwest::header::HeaderValue::from_str(&cookies)
        {
            val.set_sensitive(true);
            builder = builder.header(reqwest::header::COOKIE, val);
        }
        builder
    }

    async fn get_a_bogus_params(
        &self,
        params: &HashMap<&str, &str>,
        user_agent: &str,
    ) -> Result<String, ExtractorError> {
        let params_str = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        debug!(
            parameter_count = params.len(),
            "Signing Douyin request parameters"
        );
        let mut abogus = ABogus::new(None, Some(user_agent), None);
        let (final_params, _, _, _) = abogus.generate_abogus(&params_str, "");
        Ok(final_params)
    }

    async fn get_pc_response(&mut self) -> Result<String, ExtractorError> {
        let mut params = get_common_params();
        params.insert("web_rid", &self.web_rid);
        params.insert("cookie_enabled", "true");
        let abogus = self.get_a_bogus_params(&params, DEFAULT_UA).await?;
        let url = format!("{WEBCAST_ENTER_URL}?{abogus}");
        debug!("Prepared Douyin API request");

        let response =
            ExtractorError::check_response(self.request(reqwest::Method::GET, &url).send().await?)?;

        self.parse_and_store_cookies(response.headers());

        response.text().await.map_err(ExtractorError::from)
    }

    async fn get_app_response(&mut self) -> Result<String, ExtractorError> {
        let default_room_id = "2";
        let room_id = self.id_str.as_ref().map_or(default_room_id, |v| v.as_str());
        let sec_rid = self
            .sec_rid
            .as_deref()
            .ok_or_else(|| ExtractorError::ValidationError("Missing sec_user_id".to_string()))?;
        let mut params = HashMap::new();
        params.insert("room_id", room_id);
        params.insert("sec_user_id", sec_rid);
        params.insert("type_id", "0");
        params.insert("live_id", "1");
        params.insert("version_code", "99.99.99");
        params.insert("app_id", "1128");
        params.insert("aid", "6383");

        let verify_fp = gen_verify_fp();
        params.insert("verifyFp", verify_fp.as_str());

        let abogus = self.get_a_bogus_params(&params, DEFAULT_MOBILE_UA).await?;

        let url = format!("{APP_REFLOW_URL}?{abogus}");
        debug!("Prepared Douyin API request");

        // Build request directly so `platform_params` do not alter signed query params.
        let mut builder = self
            .config
            .extractor
            .client
            .request(reqwest::Method::GET, &url)
            .header(reqwest::header::REFERER, LIVE_DOUYIN_URL)
            .header(reqwest::header::ORIGIN, LIVE_DOUYIN_URL)
            .header(reqwest::header::USER_AGENT, DEFAULT_MOBILE_UA);

        if !self.cookies.is_empty() {
            let cookie_string = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(mut val) = reqwest::header::HeaderValue::from_str(&cookie_string) {
                val.set_sensitive(true);
                builder = builder.header(reqwest::header::COOKIE, val);
            }
        }
        debug!("Prepared Douyin request headers");
        let response = ExtractorError::check_response(builder.send().await?)?;

        self.parse_and_store_cookies(response.headers());

        response.text().await.map_err(ExtractorError::from)
    }

    fn parse_and_store_cookies(&mut self, headers: &reqwest::header::HeaderMap) {
        for value in headers.get_all("set-cookie").iter() {
            if let Ok(cookie_str) = value.to_str()
                && let Some(cookie_part) = cookie_str.split(';').next()
                && let Some((name, value)) = cookie_part.split_once('=')
            {
                let name = name.trim();
                let value = value.trim();
                if name.is_empty() || value.is_empty() {
                    continue;
                }
                self.cookies.insert(name.to_owned(), value.to_owned());
            }
        }
    }

    async fn ensure_all_requirements(&mut self) -> Result<(), ExtractorError> {
        self.ensure_ttwid().await?;
        self.ensure_odin_ttid();
        Ok(())
    }

    pub(super) async fn ensure_ttwid(&mut self) -> Result<(), ExtractorError> {
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
                    fetch_ttwid(&self.config.extractor.client).await?
                }
            }
        };
        self.cookies.insert("ttwid".to_string(), ttwid);
        Ok(())
    }

    fn ensure_odin_ttid(&mut self) {
        if self.cookies.contains_key("odin_ttid") {
            return;
        }
        let odin_ttid = generate_odin_ttid();
        self.cookies.insert("odin_ttid".to_string(), odin_ttid);
    }

    pub(super) fn parse_pc_response(&mut self, body: &str) -> Result<MediaInfo, ExtractorError> {
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty response from API".to_string(),
            ));
        }

        let trimmed = body.trim_start();
        if trimmed.starts_with("<!") || trimmed.starts_with("<html") {
            debug!(
                response_bytes = body.len(),
                "Received HTML instead of Douyin JSON"
            );
            return Err(ExtractorError::ValidationError(
                "Received HTML error page instead of JSON - API may be rate limiting or blocking requests".to_string(),
            ));
        }

        let response: DouyinPcResponse = match serde_json::from_str(body) {
            Ok(resp) => resp,
            Err(e) => {
                debug!(category = ?e.classify(), response_bytes = body.len(), "Failed to parse Douyin JSON response");
                return Err(ExtractorError::JsonError(e));
            }
        };

        if response.is_success()
            && let Some(user) = response
                .data
                .user
                .as_ref()
                .filter(|user| !user.is_cancelled())
        {
            self.platform_metadata = super::metadata::metadata(&self.web_rid, user.id_str);
        }

        if let Some(prompts) = &response.data.prompts
            && (prompts.contains("直播已结束") || prompts.contains("你不在主播设置的可见范围内"))
        {
            debug!("Douyin stream ended or visibility restricted");
            let title = response
                .data
                .data
                .as_ref()
                .and_then(|d| d.first())
                .map(|d| d.title.as_str())
                .unwrap_or("");

            let user = response.data.user.as_ref();
            let artist = user
                .map(|u| u.nickname.as_str())
                .unwrap_or("Douyin Streamer");

            let cover_url = response
                .data
                .data
                .as_ref()
                .and_then(|d| d.first())
                .and_then(|d| d.cover.as_ref())
                .and_then(|cover| cover.url_list.first())
                .map(|url| url.to_string());
            let avatar_url = user
                .as_ref()
                .and_then(|u| u.avatar_thumb.as_ref())
                .and_then(|avatar| avatar.url_list.first())
                .map(|url| url.to_string());

            return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
        }

        self.validate_response(&response)?;

        let user = response.data.user.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("User data not available".to_string())
        })?;
        self.id_str = response.data.enter_room_id.map(|v| v.to_string());
        self.sec_rid = Some(user.sec_uid.to_string());

        let data = self.extract_data(&response)?;

        self.build_media_info(data, user)
    }

    pub(super) fn parse_app_response(&mut self, body: &str) -> Result<MediaInfo, ExtractorError> {
        if body.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty response from APP API".to_string(),
            ));
        }

        let trimmed = body.trim_start();
        if trimmed.starts_with("<!") || trimmed.starts_with("<html") {
            debug!(
                response_bytes = body.len(),
                "Received HTML instead of Douyin APP JSON"
            );
            return Err(ExtractorError::ValidationError(
                "Received HTML error page from APP API".to_string(),
            ));
        }

        let response: DouyinAppResponse = match serde_json::from_str(body) {
            Ok(resp) => resp,
            Err(e) => {
                debug!(category = ?e.classify(), response_bytes = body.len(), "Failed to parse Douyin APP JSON response");
                return Err(ExtractorError::JsonError(e));
            }
        };
        if let Some(prompts) = &response.data.prompts {
            if prompts.contains("直播已结束") || prompts.contains("你不在主播设置的可见范围内")
            {
                debug!("Douyin stream ended or visibility restricted");
                let user = response.data.user.as_ref();
                let title = response
                    .data
                    .room
                    .as_ref()
                    .map(|r| r.title.as_str())
                    .unwrap_or("");
                let (artist, avatar_url) = streamer_identity(user, response.data.room.as_ref());
                let artist = artist.unwrap_or("Douyin Streamer");
                let cover_url = response
                    .data
                    .room
                    .as_ref()
                    .and_then(|r| r.cover.as_ref())
                    .and_then(|cover| cover.url_list.first())
                    .map(|url| url.to_string());

                return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
            }

            return Err(ExtractorError::ValidationError(format!(
                "API error: {prompts}",
            )));
        }

        if let Some(user) = &response.data.user {
            if user.is_cancelled() {
                return Err(ExtractorError::StreamerNotFound);
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
        let user = response.data.user.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("User data not available".to_string())
        })?;

        self.build_media_info(
            data.as_ref().ok_or_else(|| {
                ExtractorError::ValidationError("Room data not available".to_string())
            })?,
            user,
        )
    }

    fn validate_response(&self, response: &DouyinPcResponse) -> Result<(), ExtractorError> {
        if let Some(prompts) = &response.data.prompts {
            return Err(ExtractorError::ValidationError(format!(
                "API error: {prompts}"
            )));
        }
        if let Some(user) = &response.data.user {
            if user.is_cancelled() {
                return Err(ExtractorError::StreamerNotFound);
            }
        } else {
            let msg = response.data.message.as_ref().unwrap_or(&"Unknown error");
            return Err(ExtractorError::ValidationError(format!("API error: {msg}")));
        }
        Ok(())
    }

    fn extract_data<'b>(
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

    fn build_media_info(
        &self,
        data: &DouyinPcData,
        user: &DouyinUserInfo,
    ) -> Result<MediaInfo, ExtractorError> {
        // Reflow rooms can keep status=2 and playable streams after the broadcast ends
        // or while serving a replay, so `status` alone does not mean live.
        let is_live = data.status == 2 && !data.has_ended() && data.is_replay != Some(true);

        let title = &data.title;
        let (artist, avatar_url) = streamer_identity(Some(user), Some(data));
        let artist = artist.unwrap_or_default();
        let cover_url = data
            .cover
            .as_ref()
            .and_then(|cover| cover.url_list.first())
            .map(|url| url.to_string());

        if !is_live {
            return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
        }

        if self.config.skip_interactive_games && data.is_interactive_game_stream() {
            debug!("Skipping interactive game stream for room: {}", data.id_str);
            return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
        }

        let stream_url = data
            .stream_url
            .as_ref()
            .ok_or_else(|| ExtractorError::ValidationError("Stream is not live".to_string()))?;
        let mut streams = extract_streams(self.config, stream_url)?;
        let had_streams = !streams.is_empty();
        streams.retain(|stream| !stream.url.is_empty());
        if had_streams && streams.is_empty() {
            return Ok(self.create_offline_media_info(title, artist, cover_url, avatar_url));
        }
        let mut extras = FxHashMap::default();
        if let Some(id) = self
            .id_str
            .clone()
            .or_else(|| (!data.id_str.is_empty()).then(|| data.id_str.to_string()))
        {
            extras.insert("id_str".to_string(), id);
        }

        let mut info = MediaInfo::new(
            self.config.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.config.extractor.get_platform_headers_map()),
            Some(extras),
        );
        info.platform_metadata = self.platform_metadata.clone();
        Ok(info)
    }

    fn create_offline_media_info(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<String>,
        avatar_url: Option<String>,
    ) -> MediaInfo {
        let mut info = MediaInfo::new(
            self.config.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            false,
            Vec::new(),
            None,
            None,
        );
        info.platform_metadata = self.platform_metadata.clone();
        info
    }
}

/// Returns the streamer's name and avatar URL, taking each from `user` when it is set
/// and otherwise from the room owner.
fn streamer_identity<'d>(
    user: Option<&'d DouyinUserInfo<'_>>,
    room: Option<&'d DouyinPcData<'_>>,
) -> (Option<&'d str>, Option<String>) {
    let owner = room.and_then(|room| room.owner.as_ref());
    let name = user
        .map(|user| user.nickname.as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            owner
                .and_then(|owner| owner.nickname.as_deref())
                .filter(|name| !name.is_empty())
        });
    let first_url = |avatar: Option<&DouyinAvatarThumb<'_>>| {
        avatar
            .and_then(|avatar| avatar.url_list.first())
            .filter(|url| !url.is_empty())
            .map(|url| url.to_string())
    };
    let avatar = first_url(user.and_then(|user| user.avatar_thumb.as_ref()))
        .or_else(|| first_url(owner.and_then(|owner| owner.avatar_thumb.as_ref())));
    (name, avatar)
}
