use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tracing::debug;
use url::Url;

use crate::extractor::hls_extractor::HlsExtractor;
use crate::extractor::platforms::pandatv::models::{PandaTvBjResponse, PandaTvLiveResponse};
use crate::media::StreamInfo;
use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
    },
    media::MediaInfo,
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?pandalive\.co\.kr/play/([a-zA-Z0-9_-]+)").unwrap()
});

pub struct PandaTV {
    extractor: Extractor,
    _extras: Option<serde_json::Value>,
}

impl PandaTV {
    const BASE_URL: &str = "https://www.pandalive.co.kr";

    const BJ_API_URL: &str = "https://api.pandalive.co.kr/v1/member/bj";

    const LIVE_API_URL: &str = "https://api.pandalive.co.kr/v1/live/play";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("pandatv".to_string(), url, client);
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
        Self {
            extractor,
            _extras: extras,
        }
    }

    fn extract_room_id(&self) -> Result<String, ExtractorError> {
        let url = &self.extractor.url.clone();
        let caps = URL_REGEX
            .captures(url)
            .ok_or(ExtractorError::InvalidUrl(url.clone()))?;
        let room_id = caps.get(1).unwrap().as_str();
        Ok(room_id.to_string())
    }

    async fn get_room_info(&self, room_id: &str) -> Result<PandaTvBjResponse, ExtractorError> {
        let mut params = FxHashMap::default();
        params.insert("userId", room_id);
        params.insert("info", "media fanGrade");

        let response = self
            .extractor
            .post(Self::BJ_API_URL)
            .form(&params)
            .send()
            .await?
            .json::<PandaTvBjResponse>()
            .await?;

        // debug!("Response: {:?}", response);

        if !response.result {
            let msg = response.message.unwrap_or("Unknown error".to_string());
            return Err(ExtractorError::ValidationError(msg));
        }

        Ok(response)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_media_info(
        &self,
        title: String,
        artist: String,
        artist_url: Option<String>,
        cover_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
        extras: Option<FxHashMap<String, String>>,
    ) -> MediaInfo {
        MediaInfo {
            site_url: Self::BASE_URL.to_string(),
            title,
            artist,
            artist_url,
            cover_url,
            is_live,
            streams,
            extras,
        }
    }

    async fn parse_live_info(
        &self,
        response: PandaTvBjResponse,
    ) -> Result<MediaInfo, ExtractorError> {
        let bj_info = response.bj_info.ok_or(ExtractorError::ValidationError(
            "Bj info field is missing".to_string(),
        ))?;

        let title = bj_info.channel_title.to_string();
        let artist = bj_info.nick.to_string();

        let media = response.media;

        let is_live = media.is_some() && media.as_ref().unwrap().is_live;

        if !is_live {
            return Ok(self.create_media_info(title, artist, None, None, is_live, vec![], None));
        }

        let media = media.as_ref().unwrap();
        // is live
        let artist_url = media.user_img.to_string();
        let cover_url = Some(media.thumb_url.to_string());

        let url = Url::parse(&self.extractor.url).unwrap();

        let pwd = url
            .query_pairs()
            .find(|(key, _)| key == "pwd")
            .map(|(_, value)| value.to_string());

        if media.is_pw && pwd.is_none() {
            return Err(ExtractorError::PrivateContent);
        }

        let cookies = self.extractor.get_cookies();

        if cookies.is_empty() && media.is_adult {
            return Err(ExtractorError::AgeRestrictedContent);
        }

        let live_info = self.get_live_info(&media.user_id, pwd).await?;

        // debug!("Live info: {:?}", live_info);

        let hls_url = live_info
            .play_list
            .hls
            .first()
            .ok_or(ExtractorError::ValidationError(
                "HLS stream not found".to_string(),
            ))?
            .url
            .clone();

        let mut extras = FxHashMap::default();
        extras.insert("token".to_string(), live_info.token);
        // extras.insert("chat_server".to_string(), live_info.chat_server.url);
        extras.insert("t".to_string(), live_info.chat_server.t.to_string());
        extras.insert("token".to_string(), live_info.chat_server.token);
        extras.insert("rid".to_string(), bj_info.id.to_string());
        extras.extend(self.extractor.get_platform_headers_map());

        let headers = self.extractor.get_platform_headers().clone();
        let streams = self
            .extract_hls_stream::<()>(
                &self.extractor.client,
                Some(headers),
                None,
                &hls_url,
                None,
                None,
            )
            .await?;

        Ok(self.create_media_info(
            title,
            artist,
            Some(artist_url),
            cover_url,
            is_live,
            streams,
            Some(extras),
        ))
    }

    async fn get_live_info(
        &self,
        room_id: &str,
        pwd: Option<String>,
    ) -> Result<PandaTvLiveResponse, ExtractorError> {
        let mut params = FxHashMap::default();
        let pwd = pwd.unwrap_or_default();
        params.insert("userId", room_id);
        params.insert("action", "watch");
        params.insert("password", pwd.as_str());

        let response = self
            .extractor
            .post(Self::LIVE_API_URL)
            .form(&params)
            .send()
            .await?
            .json::<PandaTvLiveResponse>()
            .await?;

        if !response.result {
            return Err(ExtractorError::ValidationError(response.message));
        }

        Ok(response)
    }
}

impl HlsExtractor for PandaTV {}

#[async_trait]
impl PlatformExtractor for PandaTV {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let rid = self.extract_room_id()?;

        debug!("Room ID: {}", rid);

        let api_response = self.get_room_info(&rid).await?;

        let media_info = self.parse_live_info(api_response).await?;

        Ok(media_info)
    }
}

#[cfg(test)]
mod tests {
    use tracing::{Level, debug};

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::pandatv::builder::PandaTV,
    };

    const TEST_URL: &str = "https://www.pandalive.co.kr/play/codud23";

    #[tokio::test]
    #[ignore]
    async fn test_live_integration() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let extractor = PandaTV::new(TEST_URL.to_string(), default_client(), None, None);
        let media_info = extractor.extract().await.unwrap();
        debug!("Media info: {:?}", media_info);
    }
}
