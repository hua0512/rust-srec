use crate::extractor::error::ExtractorError;
use crate::extractor::extractor::{Extractor, PlatformExtractor};
use crate::media::media_info::MediaInfo;
use async_trait::async_trait;
use reqwest::Client;

pub struct DouyuExtractor {
    extractor: Extractor,
}

impl DouyuExtractor {
    pub fn new(platform_url: String, client: Client) -> Self {
        Self {
            extractor: Extractor::new("Douyu".to_string(), platform_url, client),
        }
    }
}

#[async_trait]
impl PlatformExtractor for DouyuExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        Err(ExtractorError::UnsupportedExtractor)
    }
}
