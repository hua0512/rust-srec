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

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?xhslink\.com/m/[a-zA-Z0-9_-]+").unwrap());
static SCRIPT_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<script>window.__INITIAL_STATE__=(.*?)</script>").unwrap());

// Constants for common strings and values
const DEFAULT_QUALITY: &str = "原画";
const DEFAULT_CODEC_H264: &str = "avc";
const DEFAULT_CODEC_H265: &str = "hevc";
const DEFAULT_QUALITY_TYPE: &str = "HD";
const M3U8_EXTENSION: &str = ".m3u8";
const FLV_EXTENSION: &str = ".flv";
const XHS_CDN_FLV_PREFIX: &str = "http://live-source-play.xhscdn.com/live/";
const USER_AGENT: &str = "ios/7.830 (ios 17.0; ; iPhone 15 (A2846/A3089/A3090/A3092))";
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
        ];

        for (key, value) in headers {
            extractor.add_header_str(key, value);
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

    fn deeplink_param(deeplink: &str, key: &str) -> Option<String> {
        let url = reqwest::Url::parse(deeplink).ok()?;
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
    }

    fn build_cdn_flv_urls_from_deeplink(deeplink: &str) -> Option<(String, String)> {
        let flv_url = Self::deeplink_param(deeplink, "flvUrl")?;
        let room_id = flv_url.split("live/").nth(1)?.split('.').next()?;
        let cdn_flv = format!("{XHS_CDN_FLV_PREFIX}{room_id}.flv");
        let cdn_m3u8 = cdn_flv.replace(FLV_EXTENSION, M3U8_EXTENSION);
        Some((cdn_flv, cdn_m3u8))
    }

    pub async fn get_live_info(&self) -> Result<MediaInfo, ExtractorError> {
        let response = self.extractor.get(&self.extractor.url).send().await?;
        let url = response.url().clone();
        debug!("redirected url: {}", url);

        let body = response.text().await?;
        let site_url = self.extractor.url.clone();

        let script_data = match Self::extract_script_data(&body) {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "RedBook: missing __INITIAL_STATE__ script");
                return Ok(MediaInfo::builder(site_url, "".to_string(), "".to_string())
                    .is_live(false)
                    .build());
            }
        };

        let live_info: LiveInfo = serde_json::from_str(&script_data)?;
        debug!("live_info: {:?}", live_info);

        let Some(live_stream) = live_info.live_stream else {
            return Ok(MediaInfo::builder(site_url, "".to_string(), "".to_string())
                .is_live(false)
                .build());
        };

        let room_data = &live_stream.room_data;

        // Extract metadata
        let artist = &room_data.host_info.nick_name;
        let avatar_url = Some(room_data.host_info.avatar.to_string());
        let site_url = self.extractor.url.clone();
        let title = format!("{artist} 的直播");
        let is_live = live_info.live_stream.live_status == SUCCESS_STATUS;

        // Validate live status
        if !is_live {
            // not live
            return Ok(MediaInfo::builder(site_url, title, artist.to_string())
                .artist_url_opt(avatar_url)
                .is_live(false)
                .build());
        }

        if title.contains("回放") {
            return Ok(MediaInfo::builder(site_url, title, artist.to_string())
                .artist_url_opt(avatar_url)
                .is_live(false)
                .build());
        }

        let Some(pull_config) = room_data.room_info.pull_config.as_ref() else {
            if let Some((cdn_flv, cdn_m3u8)) =
                Self::build_cdn_flv_urls_from_deeplink(&room_data.room_info.deeplink)
            {
                let streams = vec![
                    StreamInfo::builder(cdn_flv, StreamFormat::Flv, MediaFormat::Flv)
                        .quality(DEFAULT_QUALITY)
                        .priority(0)
                        .codec(DEFAULT_CODEC_H264)
                        .is_headers_needed(true)
                        .build(),
                    StreamInfo::builder(cdn_m3u8, StreamFormat::Hls, MediaFormat::Ts)
                        .quality(DEFAULT_QUALITY)
                        .priority(1)
                        .codec(DEFAULT_CODEC_H264)
                        .is_headers_needed(true)
                        .build(),
                ];

                return Ok(MediaInfo::new(
                    site_url,
                    title,
                    artist.to_string(),
                    Some(room_data.room_info.room_cover.to_string()),
                    avatar_url.map(|url| url.to_string()),
                    true,
                    streams,
                    Some(self.extractor.get_platform_headers_map()),
                    None,
                ));
            }

            debug!(
                live_status = %live_stream.live_status,
                page_status = %live_stream.page_status,
                error_message = %live_stream.error_message,
                "RedBook live stream missing pull_config and deeplink fallback; treating as not live"
            );
            return Ok(MediaInfo::builder(site_url, title, artist.to_string())
                .artist_url_opt(avatar_url)
                .is_live(false)
                .build());
        };

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

        if streams.is_empty()
            && let Some((cdn_flv, cdn_m3u8)) =
                Self::build_cdn_flv_urls_from_deeplink(&room_data.room_info.deeplink)
        {
            streams.push(
                StreamInfo::builder(cdn_flv, StreamFormat::Flv, MediaFormat::Flv)
                    .quality(DEFAULT_QUALITY)
                    .priority(0)
                    .codec(DEFAULT_CODEC_H264)
                    .is_headers_needed(true)
                    .build(),
            );
            streams.push(
                StreamInfo::builder(cdn_m3u8, StreamFormat::Hls, MediaFormat::Ts)
                    .quality(DEFAULT_QUALITY)
                    .priority(1)
                    .codec(DEFAULT_CODEC_H264)
                    .is_headers_needed(true)
                    .build(),
            );
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
            None,
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

    #[test]
    fn test_url_regex_matches_share_links() {
        assert!(!super::URL_REGEX.is_match("http://xhslink.com/DEnpCgb"));
        assert!(!super::URL_REGEX.is_match("https://xhslink.com/DEnpCgb"));
        assert!(super::URL_REGEX.is_match("http://xhslink.com/m/844vKmW30jz"));
        assert!(super::URL_REGEX.is_match("https://xhslink.com/m/844vKmW30jz"));

        assert!(!super::URL_REGEX.as_str().contains("xiaohongshu"));
    }

    #[test]
    fn test_build_cdn_flv_urls_from_deeplink() {
        let deeplink =
            "xhsdiscover://live?flvUrl=http%3A%2F%2Fexample.invalid%2Flive%2Froom123.flv";
        let (flv, m3u8) = super::RedBook::build_cdn_flv_urls_from_deeplink(deeplink).unwrap();
        assert_eq!(flv, "http://live-source-play.xhscdn.com/live/room123.flv");
        assert_eq!(m3u8, "http://live-source-play.xhscdn.com/live/room123.m3u8");
    }

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let redbook = RedBook::new(
            "http://xhslink.com/m/DEnpCgb".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = redbook.extract().await;
        println!("{media_info:?}");
    }
}
