use crate::extractor::error::ExtractorError;
use crate::extractor::hls_extractor::HlsExtractor;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::media::media_info::MediaInfo;
use async_trait::async_trait;
use reqwest::Client;

pub struct Twitch {
    extractor: Extractor,
}

impl Twitch {
    pub fn new(platform_url: String, client: Client, cookies: Option<String>) -> Self {
        let mut extractor = Extractor::new("Twitch".to_string(), platform_url, client);
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        Self { extractor }
    }
}

impl HlsExtractor for Twitch {}

#[async_trait]
impl PlatformExtractor for Twitch {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let m3u8_url = "http://localhost/master.m3u8";
        let streams = self
            .extract_hls_stream(&self.extractor.client, None, m3u8_url, None)
            .await?;
        let media_info = MediaInfo::new(
            self.get_extractor().url.clone(),
            "".to_string(),
            "".to_string(),
            None,
            None,
            true,
            streams,
            None,
        );
        Ok(media_info)
    }
}
