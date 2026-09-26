use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use reqwest::{
    Client,
    header::{ACCEPT, HeaderMap, HeaderValue},
};

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::redbook::{
            models::{CurrentRoomResponse, PullConfig},
            signing,
        },
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?xhslink\.com/m/[a-zA-Z0-9_-]+").unwrap());
// Constants for common strings and values
const DEFAULT_QUALITY: &str = "原画";
const DEFAULT_CODEC_H264: &str = "avc";
const DEFAULT_CODEC_H265: &str = "hevc";
const DEFAULT_QUALITY_TYPE: &str = "HD";
const M3U8_EXTENSION: &str = ".m3u8";
const FLV_EXTENSION: &str = ".flv";
const XHS_CDN_FLV_PREFIX: &str = "http://live-source-play.xhscdn.com/live/";
const USER_AGENT: &str = "ios/7.830 (ios 17.0; ; iPhone 15 (A2846/A3089/A3090/A3092))";
const ROOM_INFO_URL: &str =
    "https://live-room.xiaohongshu.com/api/sns/red/live/h5/v1/room/current_room_info";
const SHARE_SOURCE: &str = "share_out_of_app";

pub struct RedBook {
    pub extractor: Extractor,
    pub _extras: Option<serde_json::Value>,
}

/// RedBook is a social media platform that is similar to Instagram.
/// Credits to DouyinLiveRecorder for the extraction logic.
impl RedBook {
    const BASE_URL: &str = "https://app.xhs.cn";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("RedBook", url, client);
        extractor.set_origin_and_referer_static(Self::BASE_URL);
        extractor.add_header_str(reqwest::header::USER_AGENT, USER_AGENT);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        Self {
            extractor,
            _extras: extras,
        }
    }

    /// Process stream objects and convert them to StreamInfo
    fn process_streams(
        stream_objects: &[serde_json::Value],
        codec: &str,
        pull_config: &PullConfig,
        priority_offset: usize,
    ) -> Vec<StreamInfo> {
        let mut streams = Vec::new();

        for (index, stream_obj) in stream_objects.iter().enumerate() {
            if let Some(url) = stream_obj.get("master_url").and_then(|v| v.as_str()) {
                let quality = stream_obj
                    .get("quality_type_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_QUALITY);

                let format = if url.contains(M3U8_EXTENSION) || !url.contains(FLV_EXTENSION) {
                    StreamFormat::Hls
                } else {
                    StreamFormat::Flv
                };
                let is_bak = url.contains("bak");

                let display_quality = match (codec == DEFAULT_CODEC_H265, is_bak) {
                    (true, true) => format!("{quality} (H265) (backup)"),
                    (true, false) => format!("{quality} (H265)"),
                    (false, true) => format!("{quality} (backup)"),
                    (false, false) => quality.to_string(),
                };

                let extras = serde_json::json!({
                    "quality_type": stream_obj
                        .get("quality_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_QUALITY_TYPE),
                    "width": pull_config.width,
                    "height": pull_config.height
                });

                let media_format = if format == StreamFormat::Flv {
                    MediaFormat::Flv
                } else {
                    MediaFormat::Ts
                };

                streams.push(
                    StreamInfo::builder(url.to_string(), format, media_format)
                        .quality(display_quality)
                        .priority((priority_offset + index) as u32)
                        .extras(extras)
                        .codec(codec.to_string())
                        .is_headers_needed(true)
                        .build(),
                );
            }
        }

        streams
    }

    fn room_id_from_url(url: &reqwest::Url) -> Result<String, ExtractorError> {
        let trusted_host = matches!(
            url.host_str(),
            Some("www.xiaohongshu.com" | "xiaohongshu.com" | "live-room.xiaohongshu.com")
        );
        let room_id = url
            .path()
            .strip_prefix("/livestream/")
            .unwrap_or_default()
            .trim_end_matches('/');
        if trusted_host && Self::valid_room_id(room_id) {
            Ok(room_id.to_string())
        } else {
            Err(ExtractorError::ValidationError(
                "RedBook share link did not resolve to a live room".into(),
            ))
        }
    }

    fn valid_room_id(room_id: &str) -> bool {
        !room_id.is_empty()
            && room_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    fn room_info_request(&self, room_id: &str) -> Result<reqwest::RequestBuilder, ExtractorError> {
        let mut url = reqwest::Url::parse(ROOM_INFO_URL)
            .map_err(|e| ExtractorError::InvalidUrl(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("room_id", room_id)
            .append_pair("source", SHARE_SOURCE);
        // The signature covers the exact encoded path and query sent on the wire.
        let content = &url[url::Position::BeforePath..];
        let a1 = match self.extractor.cookies.get("a1") {
            Some(a1) if !a1.is_empty() => a1.as_str(),
            None if self.extractor.cookies.is_empty() => "1221",
            _ => {
                return Err(ExtractorError::ValidationError(
                    "RedBook cookies must include a non-empty a1 value".into(),
                ));
            }
        };
        let signature = signing::sign(content, a1);
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        Ok(self
            .extractor
            .get(url.as_str())
            .headers(headers)
            .header("X-s", signature))
    }

    fn parse_room_info(
        &self,
        body: &str,
        requested_room_id: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        let response: CurrentRoomResponse = serde_json::from_str(body)?;
        if !response.success {
            return Err(ExtractorError::ValidationError(format!(
                "RedBook room-info request failed (code: {:?})",
                response.code
            )));
        }
        let data = response.data.ok_or_else(|| {
            ExtractorError::ValidationError("RedBook room-info response is missing data".into())
        })?;
        let room = data.room_info.ok_or_else(|| {
            ExtractorError::ValidationError(
                "RedBook room-info response is missing room_info".into(),
            )
        })?;
        let host = data.host_info.unwrap_or_default();
        let artist = host.nick_name.unwrap_or_default();
        let title = room
            .room_title
            .as_deref()
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{artist} 的直播"));
        let is_live = room.status == Some(2)
            && room
                .room_title
                .as_deref()
                .is_some_and(|title| !title.is_empty() && !title.contains("回放"));
        let mut media = MediaInfo::builder(&self.extractor.url, title, artist)
            .artist_url_opt(host.avatar)
            .cover_url_opt(room.room_cover)
            .is_live(is_live)
            .build();
        if !is_live {
            return Ok(media);
        }

        let room_id = room
            .room_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(requested_room_id);
        if !Self::valid_room_id(room_id) {
            return Err(ExtractorError::ValidationError(
                "RedBook returned an invalid room ID".into(),
            ));
        }
        if let Some(config) = room.pull_config {
            if let Some(h264) = &config.h264 {
                media
                    .streams
                    .extend(Self::process_streams(h264, DEFAULT_CODEC_H264, &config, 0));
            }
            if let Some(h265) = &config.h265 {
                media.streams.extend(Self::process_streams(
                    h265,
                    DEFAULT_CODEC_H265,
                    &config,
                    media.streams.len(),
                ));
            }
        }
        if media.streams.is_empty() {
            media.streams = [
                (FLV_EXTENSION, StreamFormat::Flv, MediaFormat::Flv),
                (M3U8_EXTENSION, StreamFormat::Hls, MediaFormat::Ts),
            ]
            .into_iter()
            .enumerate()
            .map(|(priority, (extension, format, media_format))| {
                StreamInfo::builder(
                    format!("{XHS_CDN_FLV_PREFIX}{room_id}{extension}"),
                    format,
                    media_format,
                )
                .quality(DEFAULT_QUALITY)
                .priority(priority as u32)
                .codec(DEFAULT_CODEC_H264)
                .is_headers_needed(true)
                .build()
            })
            .collect();
        }
        media.headers = Some(self.extractor.get_platform_headers_map());
        Ok(media)
    }

    pub async fn get_live_info(&self) -> Result<MediaInfo, ExtractorError> {
        let response = self
            .extractor
            .get(&self.extractor.url)
            .send()
            .await?
            .error_for_status()?;
        let room_id = Self::room_id_from_url(response.url())?;
        // Share pages no longer need to embed __INITIAL_STATE__.
        let response = self
            .room_info_request(&room_id)?
            .send()
            .await?
            .error_for_status()?;
        self.parse_room_info(&response.text().await?, &room_id)
    }
}

#[async_trait]
impl PlatformExtractor for RedBook {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        self.get_live_info().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn extractor(cookies: Option<&str>) -> RedBook {
        RedBook::new(
            "https://xhslink.com/m/test".into(),
            crate::extractor::default::default_client(),
            cookies.map(str::to_string),
            None,
        )
    }

    fn live_response() -> Value {
        json!({
            "success": true, "code": 0,
            "data": {
                "room_info": {"room_id": "resolved123", "room_title": "a,undefined!", "status": 2, "room_cover": "https://example.invalid/cover.jpg"},
                "host_info": {"nick_name": "artist", "avatar": "https://example.invalid/avatar.jpg"}
            }
        })
    }

    #[test]
    fn test_url_regex_matches_share_links() {
        assert!(!super::URL_REGEX.is_match("http://xhslink.com/DEnpCgb"));
        assert!(!super::URL_REGEX.is_match("https://xhslink.com/DEnpCgb"));
        assert!(super::URL_REGEX.is_match("http://xhslink.com/m/844vKmW30jz"));
        assert!(super::URL_REGEX.is_match("https://xhslink.com/m/844vKmW30jz"));

        assert!(!super::URL_REGEX.as_str().contains("xiaohongshu"));
    }

    #[test]
    fn resolves_room_from_redirect_without_page_state() {
        for host in [
            "www.xiaohongshu.com",
            "xiaohongshu.com",
            "live-room.xiaohongshu.com",
        ] {
            let url = reqwest::Url::parse(&format!(
                "https://{host}/livestream/room_123-456/?host_id=user123"
            ))
            .unwrap();
            assert_eq!(RedBook::room_id_from_url(&url).unwrap(), "room_123-456");
        }
        for url in [
            "https://xhslink.com/m/test",
            "https://www.xiaohongshu.com/user/profile/user123",
            "https://www.xiaohongshu.com/livestream/",
            "https://www.xiaohongshu.com/livestream/room123/extra",
            "https://www.xiaohongshu.com/livestream/room%2F123",
            "https://example.invalid/livestream/room123",
        ] {
            assert!(
                RedBook::room_id_from_url(&reqwest::Url::parse(url).unwrap()).is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn signs_room_requests_with_and_without_cookies() {
        for cookie in [None, Some("a1=test-cookie")] {
            let request = extractor(cookie)
                .room_info_request("room123")
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(
                request.url().as_str(),
                format!("{ROOM_INFO_URL}?room_id=room123&source=share_out_of_app")
            );
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(
                request.headers()[reqwest::header::ACCEPT],
                "application/json, text/plain, */*"
            );
            let signature = request.headers()["x-s"].to_str().unwrap();
            let (content_length, digest, a1) = signing::tests::decode_request_fields(signature);
            let content = &request.url()[url::Position::BeforePath..];
            assert_eq!(content_length as usize, content.len());
            use md5::{Digest, Md5};
            assert_eq!(&digest, &Md5::digest(content.as_bytes())[..8]);
            assert_eq!(
                a1,
                if cookie.is_some() {
                    "test-cookie"
                } else {
                    "1221"
                }
            );
            assert_eq!(
                request
                    .headers()
                    .get(reqwest::header::COOKIE)
                    .map(|c| c.to_str().unwrap()),
                cookie
            );
        }
        for cookie in ["web_session=test", "a1=; web_session=test"] {
            assert!(
                extractor(Some(cookie))
                    .room_info_request("room123")
                    .is_err()
            );
        }
    }

    #[test]
    fn live_room_uses_api_metadata_and_room_id_for_both_formats() {
        let media = extractor(None)
            .parse_room_info(&live_response().to_string(), "requested123")
            .unwrap();
        assert!(media.is_live);
        assert_eq!(media.site_url, "https://xhslink.com/m/test");
        assert_eq!(media.title, "a,undefined!");
        assert_eq!(media.artist, "artist");
        assert_eq!(
            media.cover_url.as_deref(),
            Some("https://example.invalid/cover.jpg")
        );
        assert_eq!(
            media.artist_url.as_deref(),
            Some("https://example.invalid/avatar.jpg")
        );
        assert_eq!(media.streams.len(), 2);
        assert_eq!(
            media.streams[0].url,
            "http://live-source-play.xhscdn.com/live/resolved123.flv"
        );
        assert_eq!(
            media.streams[1].url,
            "http://live-source-play.xhscdn.com/live/resolved123.m3u8"
        );
        assert_eq!(media.streams[0].stream_format, StreamFormat::Flv);
        assert_eq!(media.streams[1].stream_format, StreamFormat::Hls);
        assert!(!media.headers.unwrap().contains_key("x-s"));
    }

    #[test]
    fn sparse_live_response_falls_back_to_requested_room_id() {
        for room_id in [Value::Null, json!("")] {
            let mut response = live_response();
            response["data"]["room_info"]["room_id"] = room_id;
            response["data"]["host_info"] = Value::Null;
            response["data"]["room_info"]["room_cover"] = Value::Null;
            let media = extractor(None)
                .parse_room_info(&response.to_string(), "requested123")
                .unwrap();
            assert!(media.is_live);
            assert_eq!(media.artist, "");
            assert_eq!(media.artist_url, None);
            assert_eq!(media.cover_url, None);
            assert!(media.streams[0].url.ends_with("/requested123.flv"));
        }
    }

    #[test]
    fn offline_replay_and_untitled_rooms_have_no_streams() {
        for (status, title) in [
            (json!(3), json!("live")),
            (Value::Null, json!("live")),
            (json!(2), json!("直播回放")),
            (json!(2), json!("")),
            (json!(2), Value::Null),
        ] {
            let mut response = live_response();
            response["data"]["room_info"]["status"] = status;
            response["data"]["room_info"]["room_title"] = title;
            let media = extractor(None)
                .parse_room_info(&response.to_string(), "requested123")
                .unwrap();
            assert!(!media.is_live);
            assert!(media.streams.is_empty());
            assert_eq!(media.artist, "artist");
        }
    }

    #[test]
    fn api_failures_and_malformed_responses_are_errors() {
        for response in [
            r#"{"success":false,"code":-1,"msg":"request rejected"}"#,
            r#"{"success":true}"#,
            r#"{"success":true,"data":{}}"#,
            "<html>verification required</html>",
        ] {
            assert!(
                extractor(None)
                    .parse_room_info(response, "requested123")
                    .is_err()
            );
        }
        let mut response = live_response();
        response["data"]["room_info"]["room_id"] = json!("../bad?query=1");
        assert!(
            extractor(None)
                .parse_room_info(&response.to_string(), "requested123")
                .is_err()
        );
    }

    #[test]
    fn preserves_explicit_streams_from_object_or_string_pull_config() {
        let config = json!({"width": 1920, "height": 1080, "h264": [
            {"master_url": "https://example.invalid/live.flv", "quality_type_name": "HD"}
        ], "h265": [{"master_url": "https://example.invalid/live.m3u8"}]});
        for config in [config.clone(), json!(config.to_string())] {
            let mut response = live_response();
            response["data"]["room_info"]["pull_config"] = config;
            let media = extractor(None)
                .parse_room_info(&response.to_string(), "requested123")
                .unwrap();
            assert_eq!(media.streams.len(), 2);
            assert_eq!(media.streams[0].url, "https://example.invalid/live.flv");
            assert_eq!(media.streams[1].url, "https://example.invalid/live.m3u8");
            assert_eq!(media.streams[0].priority, 0);
            assert_eq!(media.streams[1].priority, 1);
        }
        let mut response = live_response();
        response["data"]["room_info"]["pull_config"] = json!({"h264":[],"h265":[]});
        let media = extractor(None)
            .parse_room_info(&response.to_string(), "requested123")
            .unwrap();
        assert!(media.streams[0].url.ends_with("/resolved123.flv"));
    }
}
