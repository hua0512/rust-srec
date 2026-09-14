use std::{collections::HashMap, sync::LazyLock};

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use serde_json::json;
use tracing::{debug, warn};
use url::form_urlencoded;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::tiktok::{
            models::{
                ApiLiveRoomResponse, QualityInfo, RoomUserInfo, SdkParams, StreamData,
                StreamDataInfo, TiktokResponse,
            },
            utils::{TIKTOK_WEB_URL, common_query_params},
        },
        utils::{capture_group_1_or_invalid_url, extras_get_bool, extras_get_str},
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:www\.)?tiktok\.com/@([a-zA-Z0-9_.]+)/live/?(?:[?#].*)?$").unwrap()
});

pub(crate) static LIVE_INFO_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<script id="SIGI_STATE" type="application/json">(.*?)</script>"#).unwrap()
});

/// `user.status` value reported by TikTok while the account is streaming.
const USER_STATUS_LIVE: i32 = 2;

/// Which TikTok endpoint resolves room metadata and stream URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TikTokApiMode {
    /// `api-live/user/room` JSON, then the live page HTML if that fails.
    #[default]
    Auto,
    /// `GET https://www.tiktok.com/api-live/user/room/` only. Signature-free
    /// and cookie-free, so it is the preferred path.
    Web,
    /// Parse `SIGI_STATE` from `https://www.tiktok.com/@user/live` only.
    /// More exposed to bot challenges than the JSON API.
    Html,
}

impl From<&str> for TikTokApiMode {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "web" | "api" => Self::Web,
            "html" | "webhtml" | "page" => Self::Html,
            _ => Self::Auto,
        }
    }
}

impl std::fmt::Display for TikTokApiMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Web => write!(f, "web"),
            Self::Html => write!(f, "html"),
        }
    }
}

pub struct TikTok {
    pub extractor: Extractor,
    pub api_mode: TikTokApiMode,
    /// Drop every non-`origin` quality when an `origin` stream exists.
    pub force_origin_quality: bool,
}

impl TikTok {
    const BASE_URL: &str = TIKTOK_WEB_URL;
    const API_LIVE_USER_ROOM_URL: &str = "https://www.tiktok.com/api-live/user/room/";

    const DISCONTINUED_MESSAGE: &str =
        "We regret to inform you that we have discontinued operating TikTok";

    const UNEXPECTED_ERROR_MESSAGE: &str = "UNEXPECTED_EOF_WHILE_READING";

    /// `statusCode` of `api-live/user/room` for an unknown handle.
    const API_STATUS_USER_NOT_FOUND: i64 = 19881007;

    /// `LiveRoom.liveRoomStatus` values that mean the viewer is region blocked.
    const PAGE_STATUS_REGIONAL_UNAVAILABLE: i32 = -6;

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("TikTok", url, client);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        extractor.add_header_typed(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        extractor.set_origin_and_referer_static(Self::BASE_URL);

        let api_mode = extras_get_str(extras.as_ref(), "api_mode")
            .map(TikTokApiMode::from)
            .unwrap_or_default();
        let force_origin_quality =
            extras_get_bool(extras.as_ref(), "force_origin_quality").unwrap_or(false);

        Self {
            extractor,
            api_mode,
            force_origin_quality,
        }
    }

    pub fn api_mode(mut self, mode: TikTokApiMode) -> Self {
        self.api_mode = mode;
        self
    }

    pub fn force_origin_quality(mut self, force: bool) -> Self {
        self.force_origin_quality = force;
        self
    }

    /// Returns the `@handle` portion of a live URL.
    pub fn extract_room_id(&self, url: &str) -> Result<String, ExtractorError> {
        Ok(capture_group_1_or_invalid_url(&URL_REGEX, url)?.to_owned())
    }

    fn api_live_room_url(unique_id: &str) -> String {
        // The serializer borrows a non-`Send` encoder; keep it out of the
        // async body so the extractor future stays `Send`.
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (key, value) in common_query_params() {
            query.append_pair(key, &value);
        }
        query.append_pair("uniqueId", unique_id);
        query.append_pair("sourceType", "54");
        format!("{}?{}", Self::API_LIVE_USER_ROOM_URL, query.finish())
    }

    /// Resolves room info through the JSON API. Works without cookies or a
    /// request signature; `sourceType` is mandatory or the API answers with a
    /// parameter error.
    pub(crate) async fn fetch_api_live_room(
        &self,
        unique_id: &str,
    ) -> Result<RoomUserInfo, ExtractorError> {
        let url = Self::api_live_room_url(unique_id);

        let response = self
            .extractor
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?
            .error_for_status()?;
        let body = response.text().await?;
        let parsed: ApiLiveRoomResponse = serde_json::from_str(&body).map_err(|e| {
            ExtractorError::ValidationError(format!(
                "TikTok api-live response was not JSON ({e}); the request may be blocked"
            ))
        })?;

        if parsed.status_code == Self::API_STATUS_USER_NOT_FOUND {
            return Err(ExtractorError::StreamerNotFound);
        }
        match parsed.data {
            Some(info) if parsed.status_code == 0 => Ok(info),
            _ => Err(ExtractorError::ValidationError(format!(
                "TikTok api-live error {}: {}",
                parsed.status_code,
                parsed.message.unwrap_or_default()
            ))),
        }
    }

    async fn fetch_page_content(&self) -> Result<String, ExtractorError> {
        let response = self.extractor.get(&self.extractor.url).send().await?;
        let body = response.text().await?;

        if body.contains(Self::DISCONTINUED_MESSAGE) {
            return Err(ExtractorError::RegionLockedContent);
        }

        if body.contains(Self::UNEXPECTED_ERROR_MESSAGE) {
            return Err(ExtractorError::ValidationError(
                "Unexpected error while reading page content".to_string(),
            ));
        }

        Ok(body)
    }

    fn extract_json_from_html<'a>(&self, html: &'a str) -> Result<&'a str, ExtractorError> {
        capture_group_1_or_invalid_url(&LIVE_INFO_REGEX, html).map_err(|_| {
            ExtractorError::ValidationError(
                "Failed to find live info data in page. Cookies may be required.".to_string(),
            )
        })
    }

    /// Resolves room info by scraping `SIGI_STATE` from the live page.
    pub(crate) async fn fetch_html_room(&self) -> Result<RoomUserInfo, ExtractorError> {
        let body = self.fetch_page_content().await?;
        let json_str = self.extract_json_from_html(&body)?;
        let json: TiktokResponse = serde_json::from_str(json_str)?;

        let live_room = json
            .live_room
            .ok_or_else(|| ExtractorError::ValidationError("Live room not found".to_string()))?;

        // Negative page statuses are viewer-side blocks; 0..=4 describe the
        // stream lifecycle and are fine.
        if live_room.status == Self::PAGE_STATUS_REGIONAL_UNAVAILABLE {
            return Err(ExtractorError::RegionLockedContent);
        }
        if live_room.status < 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Live room unavailable (liveRoomStatus {})",
                live_room.status
            )));
        }

        live_room
            .user_info
            .ok_or_else(|| ExtractorError::ValidationError("User info not found".to_string()))
    }

    async fn fetch_room_info(&self, unique_id: &str) -> Result<RoomUserInfo, ExtractorError> {
        match self.api_mode {
            TikTokApiMode::Web => self.fetch_api_live_room(unique_id).await,
            TikTokApiMode::Html => self.fetch_html_room().await,
            TikTokApiMode::Auto => match self.fetch_api_live_room(unique_id).await {
                Ok(info) => Ok(info),
                Err(ExtractorError::StreamerNotFound) => Err(ExtractorError::StreamerNotFound),
                Err(err) => {
                    warn!(error = %err, "TikTok api-live lookup failed; falling back to page HTML");
                    self.fetch_html_room().await
                }
            },
        }
    }

    fn quality_label(quality: &QualityInfo) -> String {
        if !quality.name.is_empty() {
            return quality.name.clone();
        }
        match quality.sdk_key.as_str() {
            "origin" => "Original".to_string(),
            "ao" => "Audio".to_string(),
            other => other.to_string(),
        }
    }

    fn normalize_codec(codec: &str, fallback: &str) -> String {
        match codec.trim().to_ascii_lowercase().as_str() {
            "h264" | "avc" | "264" => "avc".to_string(),
            "h265" | "hevc" | "265" => "hevc".to_string(),
            "" => fallback.to_string(),
            other => other.to_string(),
        }
    }

    fn process_stream_data(
        &self,
        stream_data: &Option<StreamData>,
        fallback_codec: &str,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let Some(data) = stream_data else {
            return Ok(Vec::new());
        };

        if data.pull_data.stream_data.is_empty() {
            return Ok(Vec::new());
        }

        let qualities: HashMap<&str, &QualityInfo> = data
            .pull_data
            .options
            .qualities
            .iter()
            .map(|q| (q.sdk_key.as_str(), q))
            .collect();

        let stream_info: StreamDataInfo = serde_json::from_str(&data.pull_data.stream_data)
            .map_err(|e| {
                ExtractorError::ValidationError(format!("Failed to parse stream data JSON: {e}"))
            })?;

        let mut streams = Vec::with_capacity(stream_info.data.len() * 2);

        for (sdk_key, quality_info) in stream_info.data {
            let main = quality_info.main_stream;
            if main.flv.is_empty() && main.hls.is_empty() {
                continue;
            }

            let sdk_params: SdkParams = serde_json::from_str(&main.sdk_params).unwrap_or_default();
            let quality = qualities
                .get(sdk_key.as_str())
                .map(|q| Self::quality_label(q))
                .unwrap_or_else(|| match sdk_key.as_str() {
                    "origin" => "Original".to_string(),
                    "ao" => "Audio".to_string(),
                    other => other.to_string(),
                });
            // Platform levels grow with quality; StreamSelector sorts priority
            // ascending, so invert them. Unlisted keys sort last.
            let level = qualities.get(sdk_key.as_str()).map_or(0, |q| q.level);
            let priority = u32::try_from(100 - level.clamp(0, 99)).unwrap_or(100);
            let codec = Self::normalize_codec(&sdk_params.v_codec, fallback_codec);
            let is_audio_only = sdk_key == "ao";
            // `vbitrate` is already bits per second, the unit `StreamInfo` expects.
            let bitrate = sdk_params.v_bitrate;
            let extras = json!({
                "sdk_key": sdk_key,
                "resolution": sdk_params.resolution,
            });

            let create_stream =
                |url: String, stream_format: StreamFormat, media_format: MediaFormat| {
                    StreamInfo::builder(url, stream_format, media_format)
                        .quality(quality.clone())
                        .bitrate(bitrate)
                        .priority(priority)
                        .codec(codec.clone())
                        .is_audio_only(is_audio_only)
                        .extras(extras.clone())
                        .build()
                };

            if !main.flv.is_empty() {
                streams.push(create_stream(main.flv, StreamFormat::Flv, MediaFormat::Flv));
            }
            if !main.hls.is_empty() {
                streams.push(create_stream(main.hls, StreamFormat::Hls, MediaFormat::Ts));
            }
        }

        Ok(streams)
    }

    fn build_media_info(&self, info: RoomUserInfo) -> Result<MediaInfo, ExtractorError> {
        let user = &info.user;
        let headers = Some(self.get_extractor().get_platform_headers_map());
        let mut extras = FxHashMap::default();
        extras.insert("unique_id".to_string(), user.unique_id.clone());
        if !user.room_id.is_empty() {
            // Consumed by the danmu service to open the webcast chat socket.
            extras.insert("room_id".to_string(), user.room_id.clone());
        }

        let stream_details = info.stream_details.as_ref();
        let title = stream_details.map(|d| d.title.clone()).unwrap_or_default();
        let artist = if user.nickname.is_empty() {
            user.unique_id.clone()
        } else {
            user.nickname.clone()
        };

        if user.status != USER_STATUS_LIVE {
            return Ok(
                MediaInfo::builder(self.extractor.url.clone(), title, artist)
                    .artist_url(user.avatar_larger.clone())
                    .cover_url_opt(stream_details.map(|d| d.cover_url.clone()))
                    .is_live(false)
                    .headers_opt(headers)
                    .extras(extras)
                    .build(),
            );
        }

        let stream_details = stream_details.ok_or_else(|| {
            ExtractorError::ValidationError("Stream details not found".to_string())
        })?;

        let mut streams = self.process_stream_data(&stream_details.stream_data, "avc")?;
        streams.extend(self.process_stream_data(&stream_details.hevc_stream_data, "hevc")?);

        if self.force_origin_quality {
            let has_origin = streams.iter().any(|s| {
                s.extras.as_ref().and_then(|e| e.get("sdk_key")) == Some(&json!("origin"))
            });
            if has_origin {
                streams.retain(|s| {
                    s.extras.as_ref().and_then(|e| e.get("sdk_key")) == Some(&json!("origin"))
                });
            } else {
                debug!("force_origin_quality set but TikTok offered no origin stream");
            }
        }

        let is_live = !streams.is_empty();

        Ok(
            MediaInfo::builder(self.extractor.url.clone(), title, artist)
                .artist_url(user.avatar_larger.clone())
                .cover_url(stream_details.cover_url.clone())
                .live_start_time_unix(stream_details.start_time)
                .is_live(is_live)
                .streams(streams)
                .headers_opt(headers)
                .extras(extras)
                .build(),
        )
    }

    pub async fn get_live_info(&self) -> Result<MediaInfo, ExtractorError> {
        let unique_id = self.extract_room_id(&self.extractor.url)?;
        let info = self.fetch_room_info(&unique_id).await?;
        debug!(
            unique_id = %unique_id,
            room_id = %info.user.room_id,
            status = info.user.status,
            "TikTok room resolved"
        );
        self.build_media_info(info)
    }
}

#[async_trait]
impl PlatformExtractor for TikTok {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        self.get_live_info().await
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::*;
    use crate::extractor::{default::default_client, platform_extractor::PlatformExtractor};

    fn tiktok(extras: Option<serde_json::Value>) -> TikTok {
        TikTok::new(
            "https://www.tiktok.com/@test/live".to_string(),
            default_client(),
            None,
            extras,
        )
    }

    #[test]
    fn test_extract_room_id() {
        let tiktok = tiktok(None);
        assert_eq!(
            tiktok
                .extract_room_id("https://www.tiktok.com/@test/live")
                .unwrap(),
            "test"
        );
        assert_eq!(
            tiktok
                .extract_room_id("https://www.tiktok.com/@dj.ibai/live?lang=en")
                .unwrap(),
            "dj.ibai"
        );
        assert!(
            tiktok
                .extract_room_id("https://www.tiktok.com/@test")
                .is_err()
        );
    }

    #[test]
    fn api_mode_parses_from_extras() {
        assert_eq!(tiktok(None).api_mode, TikTokApiMode::Auto);
        assert_eq!(
            tiktok(Some(json!({"api_mode": "html"}))).api_mode,
            TikTokApiMode::Html
        );
        assert_eq!(
            tiktok(Some(json!({"api_mode": "WEB"}))).api_mode,
            TikTokApiMode::Web
        );
        assert_eq!(
            tiktok(Some(json!({"api_mode": "bogus"}))).api_mode,
            TikTokApiMode::Auto
        );
    }

    fn sample_room(status: i32, with_hevc: bool) -> RoomUserInfo {
        let stream_data = |codec: &str, keys: &[(&str, &str, u64, &str)]| {
            let data: serde_json::Map<String, serde_json::Value> = keys
                .iter()
                .map(|(key, res, vbitrate, suffix)| {
                    (
                        (*key).to_string(),
                        json!({
                            "main": {
                                "flv": format!("https://pull.example/{suffix}.flv"),
                                "hls": if *key == "ao" { String::new() } else { format!("https://pull.example/{suffix}.m3u8") },
                                "sdk_params": json!({"VCodec": codec, "vbitrate": vbitrate, "resolution": res}).to_string(),
                            }
                        }),
                    )
                })
                .collect();
            json!({
                "pull_data": {
                    "options": {"qualities": [
                        {"name": "Original", "sdk_key": "origin", "level": 10},
                        {"name": "720p", "sdk_key": "hd", "level": 3},
                    ]},
                    "stream_data": json!({"data": data}).to_string(),
                }
            })
        };
        let mut live_room = json!({
            "title": "Testing",
            "coverUrl": "https://cover.example/1.jpg",
            "startTime": 1789087176,
            "status": 2,
            "streamData": stream_data("h264", &[("origin", "1920x1080", 6000000, "o"), ("hd", "1280x720", 1800000, "hd"), ("ao", "", 0, "ao")]),
        });
        if with_hevc {
            live_room["hevcStreamData"] =
                stream_data("h265", &[("hd", "1280x720", 1300000, "hd5")]);
        }
        serde_json::from_value(json!({
            "user": {
                "id": "1",
                "nickname": "Tester",
                "uniqueId": "test",
                "roomId": 7684070622104374030u64,
                "status": status,
                "avatarLarger": "https://avatar.example/a.webp",
            },
            "liveRoom": live_room,
        }))
        .unwrap()
    }

    #[test]
    fn builds_streams_with_codec_quality_and_room_extras() {
        let media = tiktok(None).build_media_info(sample_room(2, true)).unwrap();
        assert!(media.is_live);
        assert_eq!(media.title, "Testing");
        assert_eq!(media.artist, "Tester");
        let extras = media.extras.as_ref().unwrap();
        assert_eq!(
            extras.get("room_id").map(String::as_str),
            Some("7684070622104374030")
        );
        assert_eq!(extras.get("unique_id").map(String::as_str), Some("test"));

        // origin flv+hls, hd flv+hls, ao flv, hevc hd flv+hls
        assert_eq!(media.streams.len(), 7);
        let origin = media
            .streams
            .iter()
            .find(|s| s.quality == "Original" && s.stream_format == StreamFormat::Flv)
            .unwrap();
        assert_eq!(origin.codec, "avc");
        assert_eq!(origin.bitrate, 6_000_000);
        assert_eq!(origin.priority, 90);
        let hevc = media.streams.iter().find(|s| s.codec == "hevc").unwrap();
        assert_eq!(hevc.quality, "720p");
        assert_eq!(hevc.priority, 97);
        let audio = media.streams.iter().find(|s| s.is_audio_only).unwrap();
        assert_eq!(audio.quality, "Audio");
        assert_eq!(audio.stream_format, StreamFormat::Flv);
    }

    #[test]
    fn force_origin_quality_keeps_only_origin() {
        let media = tiktok(Some(json!({"force_origin_quality": true})))
            .build_media_info(sample_room(2, true))
            .unwrap();
        assert_eq!(media.streams.len(), 2);
        assert!(media.streams.iter().all(|s| s.quality == "Original"));
    }

    #[test]
    fn offline_user_reports_not_live_with_room_id() {
        let media = tiktok(None)
            .build_media_info(sample_room(4, false))
            .unwrap();
        assert!(!media.is_live);
        assert!(media.streams.is_empty());
        assert_eq!(
            media
                .extras
                .as_ref()
                .and_then(|e| e.get("room_id"))
                .map(String::as_str),
            Some("7684070622104374030")
        );
    }

    #[test]
    fn api_live_not_found_maps_to_streamer_not_found() {
        let parsed: ApiLiveRoomResponse = serde_json::from_str(
            r#"{"data": null, "message": "user_not_found", "extra": {"id": "x"}, "statusCode": 19881007}"#,
        )
        .unwrap();
        assert!(parsed.data.is_none());
        assert_eq!(parsed.status_code, TikTok::API_STATUS_USER_NOT_FOUND);
    }

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let handle =
            std::env::var("TIKTOK_LIVE_HANDLE").unwrap_or_else(|_| "aljazeeraenglish".to_string());
        let mode = std::env::var("TIKTOK_API_MODE").unwrap_or_default();
        let tiktok = TikTok::new(
            format!("https://www.tiktok.com/@{handle}/live"),
            default_client(),
            None,
            Some(json!({"api_mode": mode})),
        );

        let media_info = tiktok.extract().await.unwrap();
        println!("{}", media_info.pretty_print());
    }
}
