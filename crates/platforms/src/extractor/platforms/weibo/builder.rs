use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::weibo::models::WeiboLiveInfo,
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?weibo\.com/(?:u/\d+|l/wblive/p/show/\d+:\d+)").unwrap()
});

pub struct Weibo {
    extractor: Extractor,
    _extras: Option<serde_json::Value>,
}

impl Weibo {
    const BASE_URL: &str = "https://weibo.com";
    const STATUS_API_URL: &str = "https://weibo.com/ajax/statuses/mymblog";
    const LIVE_API_URL: &str = "https://weibo.com/l/pc/anchor/live";

    const DEFAULT_COOKIES: &str = "XSRF-TOKEN=qAP-pIY5V4tO6blNOhA4IIOD; SUB=_2AkMRNMCwf8NxqwFRmfwWymPrbI9-zgzEieKnaDFrJRMxHRl-yT9kqmkhtRB6OrTuX5z9N_7qk9C3xxEmNR-8WLcyo2PM; SUBP=0033WrSXqPxfM72-Ws9jqgMF55529P9D9WWemwcqkukCduUO11o9sBqA; WBPSESS=Wk6CxkYDejV3DDBcnx2LOXN9V1LjdSTNQPMbBDWe4lO2HbPmXG_coMffJ30T-Avn_ccQWtEYFcq9fab1p5RR6PEI6w661JcW7-56BszujMlaiAhLX-9vT4Zjboy1yf2l";

    pub fn new(
        platform_url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("weibo", platform_url, client);
        extractor.set_cookies_from_string(Self::DEFAULT_COOKIES);
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

    async fn get_room_id(&self) -> Result<String, ExtractorError> {
        /*
         * Types of urls:
         * https://weibo.com/u/6034381748
         * https://weibo.com/l/wblive/p/show/1022:2321325026370190442592
         */
        let url = Url::parse(&self.extractor.url).unwrap();

        // check if 'show' is in the url
        if url.path().contains("show") {
            // Extract the room_id from the last part after 'show/'
            let room_id = url.path().split('/').next_back().ok_or_else(|| {
                ExtractorError::ValidationError("Room ID not found in URL".to_string())
            })?;
            debug!("show rid: {}", room_id);
            Ok(room_id.to_string())
        } else if url.path().contains("/u/") {
            // Extract uid from /u/uid format
            let mut uid = url.path().split('/').nth(2).ok_or_else(|| {
                ExtractorError::ValidationError("User ID not found in URL".to_string())
            })?;
            debug!("uid: {}", uid);

            // need to convert to the true uid
            let response = self
                .extractor
                .get(Self::STATUS_API_URL)
                .query(&[("uid", uid), ("page", "1"), ("feature", "0")])
                .send()
                .await?
                .json::<serde_json::Value>()
                .await?;

            // debug!("response: {:?}", response);

            for item in response["data"]["list"].as_array().unwrap() {
                if let Some(page_info) = item["page_info"].as_object() {
                    if let Some(object_type) = page_info.get("object_type") {
                        if object_type == "live" {
                            if let Some(rid) = page_info.get("object_id").and_then(|v| v.as_str()) {
                                uid = rid;
                                break;
                            }
                        }
                    }
                }
            }

            debug!("rid: {}", uid);
            Ok(uid.to_string())
        } else {
            Err(ExtractorError::InvalidUrl(self.extractor.url.clone()))
        }
    }

    async fn get_live_info(&self, room_id: &str) -> Result<MediaInfo, ExtractorError> {
        if room_id.is_empty() {
            // streamer is offline
            return Ok(MediaInfo {
                site_url: self.extractor.url.clone(),
                title: "".to_string(),
                artist: "".to_string(),
                artist_url: None,
                cover_url: None,
                is_live: false,
                streams: vec![],
                extras: None,
            });
        }

        let response = self
            .extractor
            .get(Self::LIVE_API_URL)
            .query(&[("live_id", room_id)])
            .send()
            .await?
            .json::<WeiboLiveInfo>()
            .await?;

        if response.error_code != 0 {
            return if response.msg == "LiveRoom does not exists!" {
                Err(ExtractorError::StreamerNotFound)
            } else {
                Err(ExtractorError::ValidationError(response.msg))
            };
        }

        // debug!("response: {:?}", response);

        let user_info = response.data.user_info;

        let artist = user_info.screen_name;
        let avatar_url = user_info.profile_image_url;
        let data = response.data.item;

        let title = data.desc;
        let cover_url = data.cover;
        // not live
        let is_live = data.status == 1;

        if !is_live {
            return Ok(MediaInfo {
                site_url: self.extractor.url.clone(),
                title,
                artist,
                artist_url: Some(avatar_url),
                cover_url: Some(cover_url),
                is_live: false,
                streams: vec![],
                extras: None,
            });
        }

        let flv_url = data
            .stream_info
            .pull
            .live_origin_flv_url
            .split("_")
            .next()
            .map(|s| format!("{s}.flv"));

        // debug!("flv_url: {:?}", flv_url);

        let hls_url = data
            .stream_info
            .pull
            .live_origin_hls_url
            .split("_")
            .next()
            .map(|s| format!("{s}.m3u8"));

        // debug!("hls_url: {:?}", hls_url);

        let mut streams = vec![];
        if let Some(flv_url) = flv_url {
            streams.push(StreamInfo {
                url: flv_url,
                quality: "Source".to_string(),
                stream_format: StreamFormat::Flv,
                media_format: MediaFormat::Flv,
                bitrate: 0,
                priority: 0,
                extras: None,
                codec: "hvc1".to_string(),
                fps: 0.0,
                is_headers_needed: false,
            });
        }

        if let Some(hls_url) = hls_url {
            streams.push(StreamInfo {
                url: hls_url,
                quality: "Source".to_string(),
                stream_format: StreamFormat::Hls,
                media_format: MediaFormat::Ts,
                bitrate: 0,
                priority: 0,
                extras: None,
                codec: "hvc1".to_string(),
                fps: 0.0,
                is_headers_needed: false,
            });
        }

        Ok(MediaInfo {
            site_url: self.extractor.url.clone(),
            title,
            artist,
            artist_url: Some(avatar_url),
            cover_url: Some(cover_url),
            is_live: true,
            streams,
            extras: Some(self.extractor.get_platform_headers_map()),
        })
    }
}

#[async_trait]
impl PlatformExtractor for Weibo {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let rid = self.get_room_id().await?;
        debug!("rid: {}", rid);

        let live_info = self.get_live_info(&rid).await?;
        Ok(live_info)
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::weibo::builder::Weibo,
    };

    #[tokio::test]
    #[ignore]
    async fn test_get_room_id() {
        let weibo = Weibo::new(
            "https://weibo.com/u/6034381748".to_string(),
            default_client(),
            None,
            None,
        );
        let room_id = weibo.get_room_id().await.unwrap();
        assert_eq!(room_id, "6034381748");
    }

    #[tokio::test]
    async fn test_get_room_id_show() {
        let weibo = Weibo::new(
            "https://weibo.com/l/wblive/p/show/1022:2321325185855777275969".to_string(),
            default_client(),
            None,
            None,
        );
        let room_id = weibo.get_room_id().await.unwrap();
        assert_eq!(room_id, "1022:2321325185855777275969");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_live_info() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let weibo = Weibo::new(
            "https://weibo.com/u/6124785491".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = weibo.extract().await.unwrap();
        println!("{media_info:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_live_info_rid() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();

        let weibo = Weibo::new(
            "https://weibo.com/l/wblive/p/show/1022:2321325185855777275969".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = weibo.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
