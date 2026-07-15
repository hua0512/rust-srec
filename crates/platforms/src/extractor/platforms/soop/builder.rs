use std::sync::LazyLock;

use async_trait::async_trait;
use parking_lot::RwLock;
use regex::Regex;
use reqwest::header::{COOKIE, HeaderValue, REFERER};
use reqwest::{Client, Method};
use rustc_hash::FxHashMap;
use serde_json::json;
use tracing::debug;
use url::Url;

use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::soop::auth::login_for_cookies;
use crate::extractor::platforms::soop::models::{
    SoopChannel, SoopPlayerResponse, SoopStationStatus, SoopStreamAssign, SoopViewPreset,
};
use crate::extractor::utils::{extras_get_str, merge_cookie_header_strs, merge_cookie_headers};
use crate::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:https?://)?(?:play|ch|m)\.(?:sooplive\.co\.kr|sooplive\.com|afreecatv\.com)/([a-zA-Z0-9_]+)(?:/(\d+))?",
    )
    .unwrap()
});

#[derive(Clone, Copy)]
struct PlayerRequest<'a> {
    channel_id: &'a str,
    request_type: &'a str,
    bno: &'a str,
    pwd: &'a str,
    quality: Option<&'a str>,
}

pub struct Soop {
    extractor: Extractor,
    username: Option<String>,
    password: Option<String>,
    stream_password: Option<String>,
    login_cookie: RwLock<Option<String>>,
}

impl Soop {
    const BASE_URL: &str = "https://play.sooplive.com";
    const LIVE_THUMBNAIL_BASE_URL: &str = "https://liveimg.sooplive.co.kr/m";
    const PLAYER_API_URL: &str = "https://live.sooplive.com/afreeca/player_live_api.php";
    const STATION_STATUS_URL: &str = "https://st.sooplive.com/api/get_station_status.php";

    const REQUEST_TYPE_LIVE: &str = "live";
    const REQUEST_TYPE_AID: &str = "aid";
    const RESULT_OK: i64 = 1;
    const RESULT_OFFLINE: i64 = 0;
    const RESULT_LOGIN_REQUIRED: i64 = -6;
    /// Adult / age-gate style denial; treated like login-required for video.
    const RESULT_ADULT_GATE: i64 = -8;

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let username = extras_get_str(extras.as_ref(), "username").map(ToOwned::to_owned);
        let password = extras_get_str(extras.as_ref(), "password").map(ToOwned::to_owned);
        let stream_password =
            extras_get_str(extras.as_ref(), "stream_password").map(ToOwned::to_owned);

        let mut extractor = Extractor::new("soop", url.clone(), client);
        extractor.set_origin_static(Self::BASE_URL);
        extractor.add_header_str(REFERER.as_str(), Self::sanitized_referer(&url));

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        Self {
            extractor,
            username,
            password,
            stream_password,
            login_cookie: RwLock::new(None),
        }
    }

    /// Referer sent with every SOOP API/CDN request. The query is stripped so
    /// a `?pwd=` room password never leaves via headers, and scheme-less
    /// input is normalized so `Extractor::add_header_str` gets a valid
    /// header value.
    fn sanitized_referer(url: &str) -> String {
        match Url::parse(url).or_else(|_| Url::parse(&format!("https://{url}"))) {
            Ok(mut parsed) => {
                parsed.set_query(None);
                parsed.set_fragment(None);
                parsed.to_string()
            }
            Err(_) => Self::BASE_URL.to_string(),
        }
    }

    fn extract_channel_and_bno(&self) -> Result<(String, Option<String>), ExtractorError> {
        let captures = URL_REGEX
            .captures(&self.extractor.url)
            .ok_or_else(|| ExtractorError::InvalidUrl(self.extractor.url.clone()))?;

        let channel_id = captures
            .get(1)
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| ExtractorError::InvalidUrl(self.extractor.url.clone()))?;
        let bno = captures.get(2).map(|m| m.as_str().to_owned());

        Ok((channel_id, bno))
    }

    fn parse_input_url(&self) -> Result<Url, ExtractorError> {
        if let Ok(url) = Url::parse(&self.extractor.url) {
            return Ok(url);
        }

        Url::parse(&format!("https://{}", self.extractor.url))
            .map_err(|_| ExtractorError::InvalidUrl(self.extractor.url.clone()))
    }

    fn stream_password(&self) -> Result<Option<String>, ExtractorError> {
        let url_password = self
            .parse_input_url()?
            .query_pairs()
            .find(|(key, _)| key == "pwd")
            .map(|(_, value)| value.to_string())
            .filter(|value| !value.is_empty());

        Ok(url_password.or_else(|| self.stream_password.clone()))
    }

    fn has_login_credentials(&self) -> bool {
        self.username.as_ref().is_some_and(|v| !v.is_empty())
            && self.password.as_ref().is_some_and(|v| !v.is_empty())
    }

    #[inline]
    fn needs_login(result: i64) -> bool {
        result == Self::RESULT_LOGIN_REQUIRED || result == Self::RESULT_ADULT_GATE
    }

    fn login_required_error() -> ExtractorError {
        ExtractorError::ValidationError(
            "SOOP login required - set username/password or cookies in platform config".to_string(),
        )
    }

    /// Builds the Cookie header for SOOP requests. The session obtained by
    /// `login` overrides same-named cookies from the configured store: servers
    /// honor the first occurrence of a duplicated cookie name, so a stale
    /// configured `AuthTicket` must not ride along with the fresh one.
    fn build_cookie_header(&self) -> Result<Option<HeaderValue>, ExtractorError> {
        let stored = self
            .extractor
            .get_cookies()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        let login_cookie = self.login_cookie.read().clone();
        let header = merge_cookie_headers(
            (!stored.is_empty()).then_some(stored.as_str()),
            login_cookie.as_deref(),
        );
        match header {
            None => Ok(None),
            Some(header) => HeaderValue::from_str(&header).map(Some).map_err(|e| {
                ExtractorError::ValidationError(format!("Invalid SOOP cookie header: {e}"))
            }),
        }
    }

    fn request_with_cookie(
        &self,
        method: Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, ExtractorError> {
        let mut headers = self.extractor.get_platform_headers().clone();
        if let Some(cookie_header) = self.build_cookie_header()? {
            headers.insert(COOKIE, cookie_header);
        }

        Ok(self.extractor.client.request(method, url).headers(headers))
    }

    async fn get_channel_info(
        &self,
        request: PlayerRequest<'_>,
    ) -> Result<SoopChannel, ExtractorError> {
        let api_url = Url::parse_with_params(Self::PLAYER_API_URL, &[("bjid", request.channel_id)])
            .map_err(|e| ExtractorError::ValidationError(format!("Invalid SOOP API URL: {e}")))?;

        let mut form = FxHashMap::default();
        form.insert("from_api", "0");
        form.insert("mode", "landing");
        form.insert("player_type", "html5");
        form.insert("stream_type", "common");
        form.insert("type", request.request_type);
        form.insert("bid", request.channel_id);
        form.insert("bno", request.bno);
        form.insert("pwd", request.pwd);

        if let Some(quality) = request.quality {
            form.insert("quality", quality);
        }

        let response = self
            .request_with_cookie(Method::POST, api_url.as_str())?
            .form(&form)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "SOOP player API returned HTTP {}",
                response.status()
            )));
        }

        Ok(response.json::<SoopPlayerResponse>().await?.channel)
    }

    async fn get_channel_info_with_login_retry(
        &self,
        request: PlayerRequest<'_>,
    ) -> Result<(SoopChannel, Option<String>), ExtractorError> {
        let channel = self.get_channel_info(request).await?;

        if !Self::needs_login(channel.result) {
            return Ok((channel, None));
        }

        if !self.has_login_credentials() {
            return Ok((channel, None));
        }

        let login_cookie = self.login().await?;
        *self.login_cookie.write() = Some(login_cookie.clone());
        let channel = self.get_channel_info(request).await?;

        Ok((channel, Some(login_cookie)))
    }

    async fn login(&self) -> Result<String, ExtractorError> {
        let username = self.username.as_deref().ok_or_else(|| {
            ExtractorError::ValidationError("SOOP username is not configured".to_string())
        })?;
        let password = self.password.as_deref().ok_or_else(|| {
            ExtractorError::ValidationError("SOOP password is not configured".to_string())
        })?;

        login_for_cookies(&self.extractor.client, username, password).await
    }

    /// Combine stored extractor cookies with a freshly minted login session so
    /// the app can persist a complete Cookie header after reactive login.
    fn session_cookie_header(&self, login_cookie: &str) -> String {
        let base = self
            .extractor
            .get_cookies()
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if base.is_empty() {
            login_cookie.to_string()
        } else {
            merge_cookie_header_strs(&base, login_cookie)
        }
    }

    async fn get_station_nick(&self, channel_id: &str) -> Option<String> {
        let Ok(url) = Url::parse_with_params(Self::STATION_STATUS_URL, &[("szBjId", channel_id)])
        else {
            return None;
        };

        match self.extractor.get(url.as_str()).send().await {
            Ok(response) if response.status().is_success() => response
                .json::<SoopStationStatus>()
                .await
                .ok()
                .map(|status| status.data.user_nick)
                .filter(|nick| !nick.is_empty()),
            Ok(response) => {
                debug!(
                    status = %response.status(),
                    "SOOP station status API returned non-success status"
                );
                None
            }
            Err(e) => {
                debug!(error = %e, "Failed to fetch SOOP station status");
                None
            }
        }
    }

    /// Stable SOOP profile-image CDN path, keyed by the first two characters
    /// of the lowercased channel id. Consumed as the streamer avatar via
    /// `MediaInfo.artist_url` (see `monitor::detector`'s avatar mapping).
    fn profile_image_url(channel_id: &str) -> Option<String> {
        let channel_id = channel_id.to_lowercase();
        let prefix = channel_id.get(..2)?;
        Some(format!(
            "https://profile.img.sooplive.co.kr/LOGO/{prefix}/{channel_id}/{channel_id}.jpg"
        ))
    }

    fn live_thumbnail_url(bno: &str) -> String {
        format!("{}/{bno}", Self::LIVE_THUMBNAIL_BASE_URL)
    }

    fn categories(channel: &SoopChannel) -> Option<Vec<String>> {
        let categories = channel
            .category_tags
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|category| category.trim())
            .filter(|category| !category.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        if !categories.is_empty() {
            return Some(categories);
        }

        channel
            .cate
            .as_deref()
            .map(str::trim)
            .filter(|category| !category.is_empty())
            .map(|category| vec![category.to_string()])
    }

    async fn build_offline_media_info(&self, channel_id: &str, channel: SoopChannel) -> MediaInfo {
        let categories = Self::categories(&channel);
        let artist = self
            .get_station_nick(channel_id)
            .await
            .or(channel.bjnick)
            .unwrap_or_else(|| channel_id.to_string());
        let title = channel.title.unwrap_or_default();

        MediaInfo::builder(Self::BASE_URL, title, artist)
            .artist_url_opt(Self::profile_image_url(channel_id))
            .category_opt(categories)
            .is_live(false)
            .build()
    }

    fn required_field(field: Option<String>, name: &str) -> Result<String, ExtractorError> {
        field
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ExtractorError::ValidationError(format!("SOOP {name} is missing")))
    }

    fn stream_extra<'a>(
        extras: &'a serde_json::Value,
        key: &str,
    ) -> Result<&'a str, ExtractorError> {
        extras
            .get(key)
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError(format!("Missing SOOP {key} in stream extras"))
            })
    }

    fn parse_bitrate_hint(quality: &str) -> u64 {
        let digits = quality
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();

        if digits.is_empty() {
            return 0;
        }

        digits
            .chars()
            .rev()
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
            .saturating_mul(1_000)
    }

    fn parse_resolution_hint(label: &str) -> u32 {
        label
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .max()
            .unwrap_or(0)
    }

    /// One StreamInfo per VIEWPRESET entry (minus "auto"), URL left empty for
    /// lazy resolution in `get_url`. "original" carries no digits for
    /// `parse_bitrate_hint`, so it gets a hint above every numbered preset —
    /// otherwise a `StreamSelector` min_bitrate filter would drop the best
    /// quality while keeping hd4000-style presets.
    fn build_streams(
        channel_id: &str,
        bno: &str,
        rmd: &str,
        cdn: &str,
        pwd: &str,
        presets: Vec<SoopViewPreset>,
    ) -> Vec<StreamInfo> {
        let max_hint = presets
            .iter()
            .map(|preset| Self::parse_bitrate_hint(&preset.name))
            .max()
            .unwrap_or(0);
        let original_hint = if max_hint == 0 {
            8_000_000
        } else {
            max_hint.saturating_add(1_000_000)
        };

        let mut ranked_presets = presets
            .into_iter()
            .filter(|preset| preset.name != "auto")
            .map(|preset| {
                let bitrate = if preset.name == "original" {
                    original_hint
                } else {
                    Self::parse_bitrate_hint(&preset.name)
                };
                let resolution = Self::parse_resolution_hint(&preset.label);
                (preset, bitrate, resolution)
            })
            .collect::<Vec<_>>();

        ranked_presets.sort_by(
            |(_, bitrate_a, resolution_a), (_, bitrate_b, resolution_b)| {
                bitrate_b
                    .cmp(bitrate_a)
                    .then_with(|| resolution_b.cmp(resolution_a))
            },
        );

        ranked_presets
            .into_iter()
            .enumerate()
            .map(|(priority, (preset, bitrate, _))| {
                StreamInfo::builder("", StreamFormat::Hls, MediaFormat::Ts)
                    .quality(preset.name.clone())
                    .priority(priority as u32)
                    .bitrate(bitrate)
                    .extras(json!({
                        "bid": channel_id,
                        "bno": bno,
                        "quality": preset.name,
                        "label": preset.label,
                        "rmd": rmd,
                        "cdn": cdn,
                        "pwd": pwd,
                    }))
                    .build()
            })
            .collect()
    }

    pub(crate) fn map_cdn(cdn: &str) -> &str {
        if cdn.contains("gs_cdn") {
            "gs_cdn_pc_web"
        } else if cdn.contains("lg_cdn") {
            "lg_cdn_pc_web"
        } else {
            cdn
        }
    }

    /// Prefer CHDOMAIN; fall back to CHIP (raw host or dotted IPv4 → chat-HEX host).
    pub(crate) fn resolve_chat_host(channel: &SoopChannel) -> Option<String> {
        if let Some(domain) = channel
            .chdomain
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(domain.to_ascii_lowercase());
        }

        let chip = channel.chip.as_ref()?.trim();
        if chip.is_empty() {
            return None;
        }

        let parts: Vec<&str> = chip.split('.').collect();
        if parts.len() == 4
            && parts
                .iter()
                .all(|p| p.parse::<u8>().is_ok() && !p.is_empty())
        {
            let hex: String = parts
                .iter()
                .map(|p| format!("{:02X}", p.parse::<u8>().unwrap()))
                .collect();
            return Some(format!("chat-{hex}.sooplive.com").to_ascii_lowercase());
        }

        Some(chip.to_ascii_lowercase())
    }

    fn append_aid(view_url: &str, aid: &str) -> Result<String, ExtractorError> {
        let mut url = Url::parse(view_url)
            .map_err(|e| ExtractorError::ValidationError(format!("Invalid SOOP view_url: {e}")))?;
        url.query_pairs_mut().append_pair("aid", aid);
        Ok(url.to_string())
    }

    async fn resolve_stream_url(
        &self,
        bid: &str,
        bno: &str,
        quality: &str,
        rmd: &str,
        cdn: &str,
        pwd: &str,
    ) -> Result<String, ExtractorError> {
        let request = PlayerRequest {
            channel_id: bid,
            request_type: Self::REQUEST_TYPE_AID,
            bno,
            pwd,
            quality: Some(quality),
        };

        let (channel, _) = self.get_channel_info_with_login_retry(request).await?;
        if Self::needs_login(channel.result) {
            return Err(Self::login_required_error());
        }
        if channel.result != Self::RESULT_OK {
            return Err(ExtractorError::ValidationError(format!(
                "SOOP AID request failed with result {}",
                channel.result
            )));
        }

        let aid = Self::required_field(channel.aid, "AID")?;
        let assign_url = format!("{}/broad_stream_assign.html", rmd.trim_end_matches('/'));
        let broad_key = format!("{bno}-common-{quality}-hls");
        let response = self
            .request_with_cookie(Method::GET, &assign_url)?
            .query(&[
                ("return_type", Self::map_cdn(cdn)),
                ("broad_key", broad_key.as_str()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "SOOP stream assign returned HTTP {}",
                response.status()
            )));
        }

        let stream_assign = response.json::<SoopStreamAssign>().await?;
        let view_url = stream_assign
            .view_url
            .filter(|value| !value.is_empty())
            .ok_or(ExtractorError::NoStreamsFound)?;

        Self::append_aid(&view_url, &aid)
    }
}

#[async_trait]
impl PlatformExtractor for Soop {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let (channel_id, bno_from_url) = self.extract_channel_and_bno()?;
        let stream_password = self.stream_password()?;
        let pwd = stream_password.as_deref().unwrap_or("");
        // The live-info request always sends pwd="": the room password is
        // only honored by the type=aid request (streamlink sends "" here
        // too), and BPWD in the response tells us whether one is needed.
        let request = PlayerRequest {
            channel_id: &channel_id,
            request_type: Self::REQUEST_TYPE_LIVE,
            bno: bno_from_url.as_deref().unwrap_or(""),
            pwd: "",
            // Match the official player / research client for live metadata.
            quality: Some("master"),
        };

        let (channel, login_cookie) = self.get_channel_info_with_login_retry(request).await?;
        if Self::needs_login(channel.result) {
            return Err(Self::login_required_error());
        }

        // Geo/GDPR stubs also use RESULT=0 but are not offline rooms.
        if channel.gdpr == Some(true) {
            return Err(ExtractorError::RegionLockedContent);
        }

        if channel.result == Self::RESULT_OFFLINE {
            return Ok(self.build_offline_media_info(&channel_id, channel).await);
        }

        if channel.result != Self::RESULT_OK {
            return Err(ExtractorError::ValidationError(format!(
                "SOOP player API returned result {}",
                channel.result
            )));
        }

        if channel.bpwd.as_deref() == Some("Y") && pwd.is_empty() {
            return Err(ExtractorError::PrivateContent);
        }

        let title = channel.title.clone().unwrap_or_default();
        let artist = channel.bjnick.clone().unwrap_or_else(|| channel_id.clone());
        let bjid = channel
            .bjid
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| channel_id.clone());
        let bno = Self::required_field(channel.bno.clone(), "BNO")?;
        let cover_url = Self::live_thumbnail_url(&bno);
        let rmd = Self::required_field(channel.rmd.clone(), "RMD")?;
        let cdn = channel.cdn.clone().unwrap_or_default();
        let chat_host = Self::resolve_chat_host(&channel);
        let chatno = channel.chatno.clone().filter(|s| !s.is_empty());
        let ftk = channel.ftk.clone().filter(|s| !s.is_empty());
        let chpt = channel.chpt;
        let categories = Self::categories(&channel);
        let presets = channel.viewpreset.unwrap_or_default();

        let streams = Self::build_streams(&channel_id, &bno, &rmd, &cdn, pwd, presets);

        if streams.is_empty() {
            return Err(ExtractorError::NoStreamsFound);
        }

        let mut extras = FxHashMap::default();
        extras.insert("channel_id".to_string(), channel_id.clone());
        extras.insert("bjid".to_string(), bjid);
        extras.insert("bno".to_string(), bno);
        if let Some(host) = chat_host {
            extras.insert("chdomain".to_string(), host);
        }
        if let Some(chatno) = chatno {
            extras.insert("chatno".to_string(), chatno);
        }
        if let Some(ftk) = ftk {
            extras.insert("ftk".to_string(), ftk);
        }
        if let Some(chpt) = chpt {
            extras.insert("chpt".to_string(), chpt.to_string());
        }
        if !pwd.is_empty() {
            // Forward room password into danmu JOIN secret payload.
            extras.insert("stream_password".to_string(), pwd.to_string());
        }
        // Surface freshly minted login cookies so the app can persist them.
        if let Some(login_cookie) = login_cookie.as_deref().filter(|s| !s.is_empty()) {
            extras.insert(
                "session_cookies".to_string(),
                self.session_cookie_header(login_cookie),
            );
        }

        Ok(MediaInfo::builder(Self::BASE_URL, title, artist)
            .artist_url_opt(Self::profile_image_url(&channel_id))
            .cover_url(cover_url)
            .category_opt(categories)
            .is_live(true)
            .streams(streams)
            .headers(self.extractor.get_platform_headers_map())
            .extras(extras)
            .build())
    }

    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        if !stream_info.url.is_empty() {
            return Ok(());
        }

        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Missing SOOP stream extras".to_string())
        })?;

        let bid = Self::stream_extra(extras, "bid")?;
        let bno = Self::stream_extra(extras, "bno")?;
        let quality = Self::stream_extra(extras, "quality")?;
        let rmd = Self::stream_extra(extras, "rmd")?;
        let cdn = Self::stream_extra(extras, "cdn")?;
        let pwd = Self::stream_extra(extras, "pwd")?;

        debug!(
            bid = bid,
            bno = bno,
            quality = quality,
            "Resolving SOOP stream URL"
        );

        stream_info.url = self
            .resolve_stream_url(bid, bno, quality, rmd, cdn, pwd)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::platform_extractor::PlatformExtractor;
    use crate::extractor::platforms::soop::models::SoopPlayerResponse;

    #[test]
    fn url_regex_matches_supported_urls() {
        let valid_urls = [
            "https://play.sooplive.co.kr/example",
            "https://play.sooplive.com/example/281234567",
            "https://play.afreecatv.com/example",
            "https://ch.sooplive.co.kr/example",
            "m.sooplive.co.kr/example",
        ];

        for url in valid_urls {
            assert!(URL_REGEX.is_match(url), "{url} should match");
        }

        assert!(!URL_REGEX.is_match("https://example.com/example"));
        assert!(!URL_REGEX.is_match("https://www.sooplive.com/example"));
    }

    #[test]
    fn models_deserialize_live_response() {
        let json = r#"{
            "CHANNEL": {
                "RESULT": 1,
                "BNO": "281234567",
                "BJNICK": "streamer",
                "TITLE": "live title",
                "CATE": "00040001",
                "CATEGORY_TAGS": ["스타크래프트"],
                "RMD": "https://livestream-manager.sooplive.com",
                "CDN": "gs_cdn",
                "BPWD": "N",
                "VIEWPRESET": [
                    {"label": "Original", "name": "original"},
                    {"label": "1440p", "name": "hd4000"},
                    {"label": "Auto", "name": "auto"}
                ]
            }
        }"#;

        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.result, Soop::RESULT_OK);
        assert_eq!(response.channel.bno.as_deref(), Some("281234567"));
        assert_eq!(response.channel.cate.as_deref(), Some("00040001"));
        assert_eq!(
            response.channel.category_tags.as_deref(),
            Some(["스타크래프트".to_string()].as_slice())
        );
        assert_eq!(response.channel.viewpreset.unwrap().len(), 3);
    }

    #[test]
    fn models_deserialize_offline_response() {
        let json = r#"{"CHANNEL":{"RESULT":0,"BJNICK":"streamer","TITLE":""}}"#;
        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.result, Soop::RESULT_OFFLINE);
        assert_ne!(response.channel.gdpr, Some(true));
    }

    #[test]
    fn models_deserialize_gdpr_geo_stub() {
        let json = r#"{"CHANNEL":{"geo_cc":"ES","RESULT":0,"GDPR":true}}"#;
        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.result, Soop::RESULT_OFFLINE);
        assert_eq!(response.channel.gdpr, Some(true));
    }

    #[test]
    fn models_deserialize_login_required_string_result() {
        let json = r#"{"CHANNEL":{"RESULT":"-6"}}"#;
        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.result, Soop::RESULT_LOGIN_REQUIRED);
        assert!(Soop::needs_login(response.channel.result));
        assert!(Soop::needs_login(Soop::RESULT_ADULT_GATE));
    }

    #[test]
    fn models_deserialize_chat_fields() {
        let json = r#"{
            "CHANNEL": {
                "RESULT": 1,
                "BJID": "example",
                "CHDOMAIN": "chat-AABBCCDD.sooplive.com",
                "CHPT": "8040",
                "CHATNO": "7604",
                "FTK": "fan-ticket"
            }
        }"#;
        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.chatno.as_deref(), Some("7604"));
        assert_eq!(response.channel.ftk.as_deref(), Some("fan-ticket"));
        assert_eq!(response.channel.chpt, Some(8040));
        assert_eq!(
            Soop::resolve_chat_host(&response.channel).as_deref(),
            Some("chat-aabbccdd.sooplive.com")
        );
    }

    #[test]
    fn models_deserialize_aid_response() {
        let json = r#"{"CHANNEL":{"RESULT":1,"AID":"abc.def"}}"#;
        let response: SoopPlayerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.channel.aid.as_deref(), Some("abc.def"));
    }

    #[test]
    fn maps_known_cdns() {
        assert_eq!(Soop::map_cdn("gs_cdn"), "gs_cdn_pc_web");
        assert_eq!(Soop::map_cdn("foo_lg_cdn_bar"), "lg_cdn_pc_web");
        assert_eq!(Soop::map_cdn("custom_cdn"), "custom_cdn");
    }

    #[test]
    fn appends_encoded_aid() {
        let url = Soop::append_aid("https://cdn.example.com/live.m3u8?x=1", "a b&c").unwrap();
        assert_eq!(url, "https://cdn.example.com/live.m3u8?x=1&aid=a+b%26c");
    }

    #[test]
    fn url_regex_captures_channel_without_query_and_optional_bno() {
        let caps = URL_REGEX
            .captures("https://play.sooplive.co.kr/example?pwd=secret")
            .unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "example");
        assert!(caps.get(2).is_none());

        let caps = URL_REGEX
            .captures("https://play.sooplive.com/example/281234567")
            .unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "example");
        assert_eq!(caps.get(2).unwrap().as_str(), "281234567");
    }

    #[test]
    fn url_pwd_overrides_configured_stream_password() {
        let soop = Soop::new(
            "https://play.sooplive.co.kr/example?pwd=urlpass".to_string(),
            default_client(),
            None,
            Some(json!({"stream_password": "cfgpass"})),
        );
        assert_eq!(soop.stream_password().unwrap().as_deref(), Some("urlpass"));

        let soop = Soop::new(
            "https://play.sooplive.co.kr/example".to_string(),
            default_client(),
            None,
            Some(json!({"stream_password": "cfgpass"})),
        );
        assert_eq!(soop.stream_password().unwrap().as_deref(), Some("cfgpass"));
    }

    #[test]
    fn build_streams_skips_auto_and_ranks_original_above_numbered_presets() {
        let presets = vec![
            SoopViewPreset {
                label: "자동".to_string(),
                name: "auto".to_string(),
            },
            SoopViewPreset {
                label: "원본".to_string(),
                name: "original".to_string(),
            },
            SoopViewPreset {
                label: "1440p".to_string(),
                name: "hd4000".to_string(),
            },
        ];

        let streams = Soop::build_streams(
            "example",
            "281234567",
            "https://livestream-manager.sooplive.com",
            "gs_cdn",
            "",
            presets,
        );

        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].quality, "original");
        assert_eq!(streams[1].quality, "hd4000");
        assert_eq!(streams[0].bitrate, 5_000_000);
        assert_eq!(streams[1].bitrate, 4_000_000);
        assert_eq!(streams[0].priority, 0);
        assert_eq!(streams[1].priority, 1);
        assert!(streams.iter().all(|s| s.url.is_empty()));
    }

    #[test]
    fn build_streams_ranks_real_preset_order_from_metadata() {
        let presets = vec![
            SoopViewPreset {
                label: "360p".to_string(),
                name: "sd".to_string(),
            },
            SoopViewPreset {
                label: "540p".to_string(),
                name: "hd".to_string(),
            },
            SoopViewPreset {
                label: "720p".to_string(),
                name: "hd4k".to_string(),
            },
            SoopViewPreset {
                label: "1080p".to_string(),
                name: "original".to_string(),
            },
        ];

        let streams = Soop::build_streams(
            "example",
            "281234567",
            "https://livestream-manager.sooplive.com",
            "gs_cdn",
            "",
            presets,
        );

        assert_eq!(
            streams
                .iter()
                .map(|stream| stream.quality.as_str())
                .collect::<Vec<_>>(),
            ["original", "hd4k", "hd", "sd"]
        );
        assert_eq!(
            streams
                .iter()
                .map(|stream| stream.priority)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn resolution_hint_uses_largest_numeric_component() {
        assert_eq!(Soop::parse_resolution_hint("1080p 60fps"), 1080);
        assert_eq!(Soop::parse_resolution_hint("원본"), 0);
    }

    #[test]
    fn login_cookie_overrides_stored_cookie_with_same_name() {
        let soop = Soop::new(
            "https://play.sooplive.co.kr/example".to_string(),
            default_client(),
            Some("AuthTicket=stale; other=x".to_string()),
            None,
        );

        *soop.login_cookie.write() = Some("AuthTicket=fresh; UserTicket=u1".to_string());
        let header = soop.build_cookie_header().unwrap().unwrap();
        let header = header.to_str().unwrap();

        assert!(header.contains("AuthTicket=fresh"));
        assert!(!header.contains("AuthTicket=stale"));
        assert!(header.contains("other=x"));
        assert!(header.contains("UserTicket=u1"));
        assert_eq!(header.matches("AuthTicket=").count(), 1);
    }

    #[test]
    fn referer_strips_room_password_query() {
        assert_eq!(
            Soop::sanitized_referer("https://play.sooplive.co.kr/example?pwd=secret"),
            "https://play.sooplive.co.kr/example"
        );
        assert_eq!(
            Soop::sanitized_referer("m.sooplive.co.kr/example"),
            "https://m.sooplive.co.kr/example"
        );
    }

    #[test]
    fn profile_image_url_uses_lowercased_id_prefix() {
        assert_eq!(
            Soop::profile_image_url("Example123").as_deref(),
            Some("https://profile.img.sooplive.co.kr/LOGO/ex/example123/example123.jpg")
        );
    }

    #[test]
    fn live_thumbnail_url_uses_broadcast_id() {
        assert_eq!(
            Soop::live_thumbnail_url("281234567"),
            "https://liveimg.sooplive.co.kr/m/281234567"
        );
    }

    #[test]
    fn categories_prefer_display_tags_and_fall_back_to_id() {
        let with_tags = SoopChannel {
            cate: Some("00040001".to_string()),
            category_tags: Some(vec![" 스타크래프트 ".to_string(), String::new()]),
            ..Default::default()
        };
        assert_eq!(
            Soop::categories(&with_tags),
            Some(vec!["스타크래프트".to_string()])
        );

        let without_tags = SoopChannel {
            cate: Some("00040001".to_string()),
            ..Default::default()
        };
        assert_eq!(
            Soop::categories(&without_tags),
            Some(vec!["00040001".to_string()])
        );
    }

    #[test]
    fn bitrate_hints_use_bits_per_second() {
        assert_eq!(Soop::parse_bitrate_hint("hd4000"), 4_000_000);
        assert_eq!(Soop::parse_bitrate_hint("original"), 0);
    }

    /// Live smoke against a real open room.
    ///
    /// ```text
    /// SOOP_LIVE_URL=https://play.sooplive.co.kr/<bj> \
    ///   cargo test -p platforms-parser soop::builder::tests::test_live_integration -- --ignored --nocapture
    /// ```
    ///
    /// From GDPR-blocked regions the API returns a geo stub; this test treats
    /// that as an expected environmental skip rather than a code failure.
    #[tokio::test]
    #[ignore]
    async fn test_live_integration() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let url = std::env::var("SOOP_LIVE_URL")
            .unwrap_or_else(|_| "https://play.sooplive.co.kr/example".to_string());
        let extractor = Soop::new(url, default_client(), None, None);

        match extractor.extract().await {
            Ok(media) if media.is_live => {
                assert!(
                    !media.streams.is_empty(),
                    "live extract must yield VIEWPRESET streams"
                );
                let extras = media.extras.as_ref().expect("chat extras");
                assert!(extras.contains_key("bjid") || extras.contains_key("channel_id"));
                // Chat join needs these when the room is guest-open.
                println!(
                    "live ok streams={} chatno={:?} ftk={} chdomain={:?}",
                    media.streams.len(),
                    extras.get("chatno"),
                    extras.contains_key("ftk"),
                    extras.get("chdomain")
                );

                let mut stream = media.streams[0].clone();
                extractor
                    .get_url(&mut stream)
                    .await
                    .expect("lazy AID resolve");
                assert!(
                    stream.url.contains("m3u8") || stream.url.contains("aid="),
                    "expected HLS URL with aid, got {}",
                    stream.url
                );
                println!("resolved hls={}", stream.url);
            }
            Ok(media) => {
                println!("room offline (not a failure): {media:?}");
            }
            Err(ExtractorError::ValidationError(msg))
                if msg.contains("GDPR") || msg.contains("region") =>
            {
                println!("geo/GDPR block from this network (expected skip): {msg}");
            }
            Err(e) => panic!("unexpected extract error: {e}"),
        }
    }
}
