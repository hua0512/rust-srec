use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, header};

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::acfun::{
            models::{StartPlayResponse, VideoPlayRes, VisitorLoginResponse},
            utils::get_random_name,
        },
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:https?://)?(?:(?:www|m|live)\.)?acfun\.cn/(?:live/)?(\d+)").unwrap()
});

pub struct Acfun {
    pub extractor: Extractor,
}

impl Acfun {
    const VISITOR_LOGIN_URL: &str = "https://id.app.acfun.cn/rest/app/visitor/login";
    const START_PLAY_URL: &str = "https://api.kuaishouzt.com/rest/zt/live/web/startPlay";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        _extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Acfun", url, client);
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        Self { extractor }
    }

    fn extract_rid(&self) -> Result<&str, ExtractorError> {
        let caps = URL_REGEX.captures(&self.extractor.url);
        caps.and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or(ExtractorError::InvalidUrl(self.extractor.url.clone()))
    }
}

#[async_trait]
impl PlatformExtractor for Acfun {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let rid = self.extract_rid()?;

        let did = format!("web_{}", get_random_name(16));

        let response = self
            .extractor
            .client
            .post(Self::VISITOR_LOGIN_URL)
            .form(&[("sid", "acfun.api.visitor")])
            .header(header::COOKIE, format!("_did={did};"))
            .send()
            .await?
            .json::<VisitorLoginResponse>()
            .await?;

        if response.result != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to login: {}",
                response.result
            )));
        }

        let params = [
            ("subBiz", "mainApp".to_string()),
            ("kpn", "ACFUN_APP".to_string()),
            ("kpf", "PC_WEB".to_string()),
            ("userId", response.user_id.to_string()),
            ("did", did),
            ("acfun.api.visitor_st", response.visitor_st),
        ];
        let start_play_response = self
            .extractor
            .client
            .post(Self::START_PLAY_URL)
            .header(header::REFERER, "https://live.acfun.cn/")
            .query(&params)
            .form(&[
                ("authorId", rid.to_string()),
                ("pullStreamType", "FLV".to_string()),
            ])
            .send()
            .await?
            .json::<StartPlayResponse>()
            .await?;

        if start_play_response.result != 1 {
            return Ok(MediaInfo {
                title: "".to_string(),
                is_live: false,
                streams: vec![],
                site_url: self.extractor.url.clone(),
                artist: "".to_string(),
                artist_url: None,
                cover_url: None,
                extras: None,
            });
        }

        let data = start_play_response.data.ok_or_else(|| {
            ExtractorError::ValidationError("No data found in start play response".to_string())
        })?;

        let video_play_res: VideoPlayRes = serde_json::from_str(&data.video_play_res)?;

        let streams = video_play_res
            .live_adaptive_manifest
            .into_iter()
            .flat_map(|manifest| manifest.adaptation_set.representation)
            .map(|rep| StreamInfo {
                url: rep.url,
                stream_format: StreamFormat::Flv,
                media_format: MediaFormat::Flv,
                quality: rep.name,
                bitrate: rep.bitrate as u64,
                priority: rep.level,
                extras: None,
                codec: rep.media_type,
                fps: 0.0,
                is_headers_needed: false,
            })
            .collect::<Vec<_>>();

        if streams.is_empty() {
            return Err(ExtractorError::NoStreamsFound);
        }

        Ok(MediaInfo {
            title: data.caption,
            is_live: true,
            streams,
            site_url: self.extractor.url.clone(),
            artist: "".to_string(),
            artist_url: None,
            cover_url: None,
            extras: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        let client = Client::new();
        let extractor = Acfun::new(
            "https://live.acfun.cn/live/265502".to_string(),
            client,
            None,
            None,
        );
        let media_info = extractor.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
