use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use tracing::debug;

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?(?:www\.)?picarto\.tv/([a-zA-Z0-9_-]+)").unwrap());

use crate::{
    extractor::{
        error::ExtractorError,
        hls_extractor::HlsExtractor,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::picarto::models::PicartoResponse,
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub struct Picarto {
    pub extractor: Extractor,
}

impl Picarto {
    const BASE_URL: &'static str = "https://picarto.tv";

    const API_URL_LIVE: &'static str = "https://ptvintern.picarto.tv/api/channel/detail/";

    const HLS_URL: &'static str = "{netloc}/stream/hls/{file_name}/index.m3u8";

    const MP4_URL: &'static str = "{netloc}/stream/{file_name}.mp4?secret=";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        _extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Picarto", url, client);
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
        Self { extractor }
    }

    pub fn extract_room_id(&self) -> Result<String, ExtractorError> {
        // Extract the room ID from the URL
        let url = self.extractor.url.clone();
        let room_id = url
            .split('/')
            .next_back()
            .ok_or_else(|| ExtractorError::ValidationError("Room ID not found in URL".to_string()))?
            .to_string();
        Ok(room_id)
    }

    pub async fn get_live_info(&self, rid: &str) -> Result<MediaInfo, ExtractorError> {
        // This function should implement the logic to get live info from Picarto
        // For now, we return a placeholder value

        let api_url = format!("{}{}", Self::API_URL_LIVE, rid);
        let response = self.extractor.get(&api_url).send().await?;

        let data = response.json::<PicartoResponse>().await?;

        debug!("Picarto response: {:?}", data);

        if data.channel.is_none() {
            return Err(ExtractorError::StreamerNotFound);
        }

        let channel = data.channel.unwrap();
        let id = channel.id;

        let artist = channel.name;
        let title = channel.title;
        let avatar_url = channel.avatar;
        let is_live = channel.streaming;

        if !is_live {
            return Ok(MediaInfo {
                site_url: self.extractor.url.clone(),
                title,
                artist,
                cover_url: None,
                artist_url: Some(avatar_url),
                is_live,
                streams: vec![],
                extras: None,
            });
        }

        if data.load_balancer.is_none() {
            return Err(ExtractorError::ValidationError(
                "Load balancer not found".to_string(),
            ));
        }

        if data.get_multi_streams.is_none() {
            return Err(ExtractorError::ValidationError(
                "Get multi streams not found".to_string(),
            ));
        }

        let load_balancer = data.load_balancer.unwrap();
        let get_multi_streams = data.get_multi_streams.unwrap();

        let stream = get_multi_streams
            .streams
            .iter()
            .find(|stream| stream.id == id)
            .ok_or_else(|| ExtractorError::NoStreamsFound)?;

        let mp4_url = Self::MP4_URL
            .replace("{netloc}", &load_balancer.url)
            .replace("{file_name}", &stream.stream_name);
        let mp4_url = mp4_url.replace("http://", "https://");

        let hls_url = Self::HLS_URL
            .replace("{netloc}", &load_balancer.url)
            .replace("{file_name}", &stream.stream_name);
        let hls_url = hls_url.replace("http://", "https://");

        debug!("HLS URL: {}", hls_url);

        let headers = self.extractor.get_platform_headers().clone();
        let mut streams = self
            .extract_hls_stream(&self.extractor.client, Some(headers), &hls_url, None, None)
            .await?;

        streams.push(StreamInfo {
            url: mp4_url,
            stream_format: StreamFormat::Mp4,
            media_format: MediaFormat::Mp4,
            quality: "Source".to_string(),
            bitrate: 0,
            priority: 0,
            extras: None,
            codec: "".to_string(),
            fps: 0.0,
            is_headers_needed: false,
        });

        Ok(MediaInfo {
            site_url: self.extractor.url.clone(),
            title,
            artist,
            cover_url: None,
            artist_url: Some(avatar_url),
            is_live,
            streams,
            extras: Some(self.extractor.get_platform_headers_map()),
        })
    }
}

impl HlsExtractor for Picarto {}

#[async_trait::async_trait]
impl PlatformExtractor for Picarto {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        // Implement the extraction logic here
        let room_id = self.extract_room_id()?;
        debug!("Extracted room ID: {}", room_id);

        let media_info = self.get_live_info(&room_id).await?;
        Ok(media_info)
    }
}

#[cfg(test)]
mod tests {

    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor, platforms::picarto::Picarto,
    };

    #[tokio::test]
    #[ignore]
    async fn test_picarto_extractor() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let picarto = Picarto::new(
            "https://picarto.tv/RaptorARTStudios".to_string(),
            default_client(),
            None,
            None,
        );

        let media_info = picarto.extract().await;
        println!("{media_info:?}");
    }
}
