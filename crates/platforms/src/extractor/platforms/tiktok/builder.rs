use std::{collections::HashMap, sync::LazyLock};

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::json;
use tracing::debug;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::tiktok::models::{SdkParams, StreamData, StreamDataInfo, TiktokResponse},
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:www\.)?tiktok\.com/@([a-zA-Z0-9_.]+)/live/?$").unwrap()
});

pub(crate) static LIVE_INFO_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<script id="SIGI_STATE" type="application/json">(.*?)</script>"#).unwrap()
});

pub struct TikTok {
    pub extractor: Extractor,
}

impl TikTok {
    const BASE_URL: &str = "https://www.tiktok.com";

    const DISCONTINUED_MESSAGE: &str =
        "We regret to inform you that we have discontinued operating TikTok";

    const UNEXPECTED_ERROR_MESSAGE: &str = "UNEXPECTED_EOF_WHILE_READING";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        _extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("TikTok", url, client);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        // Use static string slices for header names to avoid allocations
        extractor.add_header(
            reqwest::header::ACCEPT_LANGUAGE.as_str().to_owned(),
            "en-US,en;q=0.9".to_owned(),
        );
        extractor.add_header(reqwest::header::REFERER.as_str().to_owned(), Self::BASE_URL);
        extractor.add_header(reqwest::header::ORIGIN.as_str().to_owned(), Self::BASE_URL);

        Self { extractor }
    }

    pub fn extract_room_id(&self, url: &str) -> Result<String, ExtractorError> {
        if let Some(captures) = URL_REGEX.captures(url) {
            return Ok(captures.get(1).unwrap().as_str().to_string());
        }
        Err(ExtractorError::InvalidUrl(url.to_string()))
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
        LIVE_INFO_REGEX
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError(
                    "Failed to find live info data in page. Cookies may be required.".to_string(),
                )
            })
    }

    fn process_stream_data(
        &self,
        stream_data: &Option<StreamData>,
        codec: &str,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let Some(data) = stream_data else {
            return Ok(Vec::new());
        };

        let quality_map: HashMap<&str, &str> = data
            .pull_data
            .options
            .qualities
            .iter()
            .map(|q| (q.sdk_key.as_str(), q.name.as_str()))
            .collect();

        let stream_info: StreamDataInfo = serde_json::from_str(&data.pull_data.stream_data)
            .map_err(|e| {
                ExtractorError::ValidationError(format!("Failed to parse stream data JSON: {e}"))
            })?;

        let mut streams = Vec::with_capacity(stream_info.data.len() * 2); // Pre-allocate for FLV + HLS
        let codec_string = codec.to_string(); // Convert once instead of repeatedly

        for (sdk_key, quality_info) in stream_info.data {
            let quality_name = quality_map
                .get(sdk_key.as_str())
                .copied()
                .unwrap_or(&sdk_key);
            let quality_id = format!("{quality_name} - {codec}");

            let bitrate = serde_json::from_str::<SdkParams>(&quality_info.main_stream.sdk_params)
                .map(|params| params.v_bitrate / 1000)
                .unwrap_or(0);

            // Helper closure to create stream info
            let create_stream =
                |url: String, stream_format: StreamFormat, media_format: MediaFormat| StreamInfo {
                    url,
                    stream_format,
                    media_format,
                    quality: quality_id.clone(),
                    bitrate,
                    priority: 0,
                    extras: Some(json!({
                        "sdk_key": &sdk_key,
                    })),
                    codec: codec_string.clone(),
                    fps: 0.0,
                    is_headers_needed: false,
                };

            // Add FLV stream if available
            if !quality_info.main_stream.flv.is_empty() {
                streams.push(create_stream(
                    quality_info.main_stream.flv,
                    StreamFormat::Flv,
                    MediaFormat::Flv,
                ));
            }

            // Add HLS stream if available
            if !quality_info.main_stream.hls.is_empty() {
                streams.push(create_stream(
                    quality_info.main_stream.hls,
                    StreamFormat::Hls,
                    MediaFormat::Ts,
                ));
            }
        }

        Ok(streams)
    }

    pub async fn get_live_info(&self) -> Result<MediaInfo, ExtractorError> {
        let body = self.fetch_page_content().await?;
        let json_str = self.extract_json_from_html(&body)?;
        let json: TiktokResponse = serde_json::from_str(json_str)?;

        debug!("API Response: {:?}", json);

        let live_room = json
            .live_room
            .as_ref()
            .ok_or_else(|| ExtractorError::ValidationError("Live room not found".to_string()))?;

        if live_room.status != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Live room status is not 0: {}",
                live_room.status
            )));
        }

        let user_info = live_room
            .user_info
            .as_ref()
            .ok_or_else(|| ExtractorError::ValidationError("User info not found".to_string()))?;

        let user = &user_info.user;

        let is_live = user.status == 2;

        let mut media_info = MediaInfo {
            site_url: self.extractor.url.clone(),
            title: String::from(""),
            artist: user.nickname.clone(),
            artist_url: Some(user.avatar_larger.clone()),
            is_live,
            streams: Vec::new(),
            cover_url: None,
            extras: Some(self.get_extractor().get_platform_headers_map()),
        };

        if !is_live {
            return Ok(media_info);
        }

        let stream_details = user_info.stream_details.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Stream details not found".to_string())
        })?;

        media_info.title = stream_details.title.clone();

        media_info.cover_url = Some(stream_details.cover_url.clone());

        // Process streams only if live
        let mut streams = self.process_stream_data(&stream_details.stream_data, "avc")?;
        let hevc_streams = self.process_stream_data(&stream_details.hevc_stream_data, "hevc")?;
        streams.extend(hevc_streams);

        if streams.is_empty() {
            media_info.is_live = false;
        } else {
            media_info.streams = streams;
        }

        Ok(media_info)
    }
}

#[async_trait]
impl PlatformExtractor for TikTok {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let room_id = self.extract_room_id(&self.extractor.url)?;
        debug!("room_id: {}", room_id);

        self.get_live_info().await
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor, platforms::tiktok::TikTok,
    };

    #[test]
    fn test_extract_room_id() {
        let tiktok = TikTok::new(
            "https://www.tiktok.com/@test/live".to_string(),
            default_client(),
            None,
            None,
        );
        let room_id = tiktok
            .extract_room_id("https://www.tiktok.com/@test/live")
            .unwrap();
        assert_eq!(room_id, "test");
    }

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let tiktok = TikTok::new(
            "https://www.tiktok.com/@seraph_venusttv/live".to_string(),
            default_client(),
            None,
            None,
        );

        let media_info = tiktok.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
