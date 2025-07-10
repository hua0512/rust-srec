use crate::{
    extractor::{
        error::ExtractorError,
        hls_extractor::HlsExtractor,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::twitcasting::models::{StreamContainer, TwitcastingData},
    },
    media::{MediaInfo, stream_info::StreamInfo},
};
use async_trait::async_trait;
use md5::{Digest, Md5};
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use std::sync::LazyLock;
use url::Url;

// Constants
const BASE_URL: &str = "https://twitcasting.tv";
const STREAM_SERVER_API: &str = "https://twitcasting.tv/streamserver.php";
const DEFAULT_PARAMS: &[(&str, &str)] = &[("mode", "client"), ("player", "pc_web")];

// Stream quality constants
const QUALITY_HIGH: &str = "High";
const QUALITY_MEDIUM: &str = "Medium";
const QUALITY_LOW: &str = "Low";

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?twitcasting\.tv/([a-zA-Z0-9_-]+)").unwrap()
});

pub struct Twitcasting {
    pub extractor: Extractor,
}

impl Twitcasting {
    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        _extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Twitcasting", url, client);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        extractor.add_header(
            reqwest::header::ACCEPT_LANGUAGE.to_string(),
            "en-US, en;q=0.9",
        );

        extractor.add_header(reqwest::header::REFERER.to_string(), BASE_URL);

        Self { extractor }
    }

    /// Extract room ID from URL without cloning
    pub fn extract_room_id(&self) -> Result<&str, ExtractorError> {
        URL_REGEX
            .captures(&self.extractor.url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::InvalidUrl("Room ID not found in URL".to_string()))
    }

    /// Build query parameters for API request
    fn build_query_params<'a>(
        &self,
        rid: &'a str,
        pass_hash: Option<&'a str>,
    ) -> Vec<(&'a str, &'a str)> {
        let mut params = Vec::with_capacity(DEFAULT_PARAMS.len() + 2);
        params.push(("target", rid));
        params.extend_from_slice(DEFAULT_PARAMS);

        if let Some(hash) = pass_hash {
            params.push(("word", hash));
        }

        params
    }

    /// Extract password hash from URL query parameters
    fn extract_password_hash(&self) -> Result<Option<String>, ExtractorError> {
        let url = Url::parse(&self.extractor.url)
            .map_err(|e| ExtractorError::InvalidUrl(format!("Failed to parse URL: {e}")))?;

        let query_params = url.query_pairs().collect::<FxHashMap<_, _>>();

        Ok(query_params.get("password").map(|pass_param| {
            let mut md5 = Md5::new();
            md5.update(pass_param.as_bytes());
            format!("{:x}", md5.finalize())
        }))
    }

    /// Fetch live data from Twitcasting API
    async fn fetch_live_data(&self, rid: &str) -> Result<TwitcastingData, ExtractorError> {
        let pass_hash = self.extract_password_hash()?;
        let params = self.build_query_params(rid, pass_hash.as_deref());

        let response = self
            .extractor
            .client
            .get(STREAM_SERVER_API)
            .query(&params)
            .send()
            .await
            .map_err(ExtractorError::HttpError)?;

        let body = response.text().await.map_err(ExtractorError::HttpError)?;

        serde_json::from_str::<TwitcastingData>(&body).map_err(ExtractorError::JsonError)
    }

    /// Create MediaInfo for non-live stream
    fn create_offline_media_info(&self, rid: &str) -> MediaInfo {
        MediaInfo {
            site_url: self.extractor.url.clone(),
            title: String::new(),
            artist: rid.to_string(),
            cover_url: None,
            artist_url: None,
            is_live: false,
            streams: vec![],
            extras: None,
        }
    }

    /// Extract all available HLS streams
    async fn extract_all_hls_streams(
        &self,
        stream_container: &StreamContainer,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let streams = &stream_container.streams;
        let mut stream_info = Vec::new();

        // Define stream qualities and their corresponding URLs
        let stream_configs = [
            (streams.high.as_deref(), QUALITY_HIGH),
            (streams.medium.as_deref(), QUALITY_MEDIUM),
            (streams.low.as_deref(), QUALITY_LOW),
        ];

        for (url, quality) in stream_configs {
            if let Some(stream_url) = url {
                let info = self
                    .extract_hls_stream::<()>(
                        &self.extractor.client,
                        None,
                        None,
                        stream_url,
                        Some(quality),
                        None,
                    )
                    .await?;
                stream_info.extend(info);
            }
        }

        Ok(stream_info)
    }

    /// Get live stream information
    pub async fn get_live_info(&self, rid: &str) -> Result<MediaInfo, ExtractorError> {
        let data = self.fetch_live_data(rid).await?;

        if !data.movie.live {
            return Ok(self.create_offline_media_info(rid));
        }

        let hls_container = data.tc_hls.ok_or_else(|| ExtractorError::NoStreamsFound)?;

        let streams_info = self.extract_all_hls_streams(&hls_container).await?;

        // TODO: Support wss streams (fmp4) - could be added here in the future

        Ok(MediaInfo {
            site_url: self.extractor.url.clone(),
            title: String::new(),
            artist: rid.to_string(),
            cover_url: None,
            artist_url: None,
            is_live: true,
            streams: streams_info,
            extras: None,
        })
    }
}

impl HlsExtractor for Twitcasting {}

#[async_trait]
impl PlatformExtractor for Twitcasting {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let rid = self.extract_room_id()?;
        self.get_live_info(rid).await
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::twitcasting::Twitcasting,
    };

    #[tokio::test]
    #[ignore]
    async fn test_twitcasting_extractor() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .try_init();

        let url = "https://twitcasting.tv/nodasori2525";
        let extractor = Twitcasting::new(url.to_string(), default_client(), None, None);
        let media_info = extractor.extract().await.unwrap();
        println!("{media_info:?}");
    }

    #[test]
    fn test_room_id_extraction() {
        let url = "https://twitcasting.tv/nodasori2525";
        let client = default_client();
        let extractor = Twitcasting::new(url.to_string(), client, None, None);

        let room_id = extractor.extract_room_id().unwrap();
        assert_eq!(room_id, "nodasori2525");
    }
}
