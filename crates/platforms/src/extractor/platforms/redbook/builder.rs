use std::collections::HashMap;

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use tracing::debug;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::redbook::models::{LiveInfo, PullConfig},
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:(?:www\.)?xiaohongshu\.com/user/profile/([a-zA-Z0-9_-]+)|xhslink\.com/[a-zA-Z0-9_-]+)")
        .unwrap()
});
static USER_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/user/profile/([^/?]*)").unwrap());

static SCRIPT_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<script>window.__INITIAL_STATE__=(.*?)</script>").unwrap());

// Constants for common strings and values
const DEFAULT_QUALITY: &str = "原画";
const DEFAULT_CODEC_H264: &str = "avc";
const DEFAULT_CODEC_H265: &str = "hevc";
const DEFAULT_QUALITY_TYPE: &str = "HD";
const M3U8_EXTENSION: &str = ".m3u8";
const FLV_EXTENSION: &str = ".flv";
const USER_AGENT: &str = "ios/7.830 (ios 17.0; ; iPhone 15 (A2846/A3089/A3090/A3092))";
const XY_COMMON_PARAMS: &str = "platform=iOS&sid=session.1722166379345546829388";
const SUCCESS_STATUS: &str = "success";

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
        Self::setup_headers(&mut extractor);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        Self {
            extractor,
            _extras: extras,
        }
    }

    /// Setup common headers for RedBook requests
    fn setup_headers(extractor: &mut Extractor) {
        let headers = [
            (reqwest::header::ORIGIN.as_str(), Self::BASE_URL),
            (reqwest::header::REFERER.as_str(), Self::BASE_URL),
            (reqwest::header::USER_AGENT.as_str(), USER_AGENT),
            ("xy-common-params", XY_COMMON_PARAMS),
        ];

        for (key, value) in headers {
            extractor.add_header(key.to_string(), value.to_string());
        }
    }

    /// Determine MediaFormat from URL extension
    fn get_format_from_url(url: &str) -> StreamFormat {
        if url.contains(M3U8_EXTENSION) {
            StreamFormat::Hls
        } else if url.contains(FLV_EXTENSION) {
            StreamFormat::Flv
        } else {
            StreamFormat::Hls // Default to HLS for unknown formats
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
                debug!("stream_obj: {:?}", stream_obj);

                let quality = stream_obj
                    .get("quality_type_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_QUALITY);

                let format = Self::get_format_from_url(url);
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

                streams.push(StreamInfo {
                    url: url.to_string(),
                    stream_format: format,
                    media_format,
                    quality: display_quality,
                    bitrate: 0,
                    priority: (priority_offset + index) as u32,
                    codec: codec.to_string(),
                    fps: 0.0,
                    is_headers_needed: true,
                    extras: Some(extras),
                });
            }
        }

        streams
    }

    /// Extract host_id from redirected URL parameters
    fn extract_host_id(url: &reqwest::Url) -> Result<String, ExtractorError> {
        let params = url.query_pairs().collect::<HashMap<_, _>>();
        params
            .get("host_id")
            .map(|id| id.to_string())
            .ok_or_else(|| {
                ExtractorError::ValidationError(
                    "RedBook: failed to extract host_id from the redirected url".into(),
                )
            })
    }

    /// Extract user_id from page body or fallback to host_id
    fn extract_user_id(body: &str, host_id: &str) -> String {
        USER_ID_REGEX
            .captures(body)
            .and_then(|captures| captures.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| host_id.to_string())
    }

    /// Extract and parse script data from page body
    fn extract_script_data(body: &str) -> Result<String, ExtractorError> {
        SCRIPT_DATA_REGEX
            .captures(body)
            .and_then(|captures| captures.get(1))
            .map(|m| m.as_str().replace("undefined", "null"))
            .filter(|data| !data.is_empty())
            .ok_or_else(|| {
                ExtractorError::ValidationError(
                    "Failed to extract script_data from the body".into(),
                )
            })
    }

    pub async fn get_live_info(&self) -> Result<MediaInfo, ExtractorError> {
        // Get redirected URL and extract host_id
        let response = self.extractor.get(&self.extractor.url).send().await?;
        let url = response.url().clone();
        debug!("redirected url: {}", url);

        let body = response.text().await?;
        let host_id = Self::extract_host_id(&url)?;
        debug!("host_id: {}", host_id);

        let user_id = Self::extract_user_id(&body, &host_id);
        debug!("user_id: {}", user_id);

        // Extract and parse live info
        let script_data = Self::extract_script_data(&body)?;
        // debug!("script_data: {}", script_data);
        let live_info: LiveInfo = serde_json::from_str(&script_data)?;
        debug!("live_info: {:?}", live_info);

        let room_data = &live_info.live_stream.room_data;
        let pull_config = room_data.room_info.pull_config.as_ref().unwrap();

        // Extract metadata
        let artist = &room_data.host_info.nick_name;
        let avatar_url = Some(room_data.host_info.avatar.to_string());
        let site_url = self.extractor.url.clone();
        let title = format!("{artist} 的直播");
        let is_live = live_info.live_stream.live_status == SUCCESS_STATUS;

        // Validate live status
        if !is_live {
            // not live
            return Ok(MediaInfo {
                site_url,
                title: format!("{artist} 的直播"),
                artist: artist.to_string(),
                cover_url: None,
                artist_url: avatar_url,
                is_live: false,
                streams: Vec::new(),
                extras: None,
            });
        }

        // Build streams from both h264 and h265 arrays
        let mut streams = Vec::new();

        // Process H264 streams
        if let Some(h264) = &pull_config.h264 {
            streams.extend(Self::process_streams(
                h264,
                DEFAULT_CODEC_H264,
                pull_config,
                0,
            ));
        }

        // Process H265 streams
        if let Some(h265) = &pull_config.h265 {
            streams.extend(Self::process_streams(
                h265,
                DEFAULT_CODEC_H265,
                pull_config,
                h265.len(),
            ));
        }

        Ok(MediaInfo::new(
            site_url,
            title,
            artist.to_string(),
            Some(room_data.room_info.room_cover.to_string()),
            avatar_url.map(|url| url.to_string()),
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
        ))
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
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::redbook::builder::RedBook,
    };

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let redbook = RedBook::new(
            "http://xhslink.com/DEnpCgb".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = redbook.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
