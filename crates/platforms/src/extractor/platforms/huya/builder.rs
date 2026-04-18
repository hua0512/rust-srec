use std::sync::LazyLock;

use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::huya::get_anticode;
use crate::extractor::platforms::huya::sign::HuyaPlatform;

use crate::extractor::utils::{extras_get_bool, extras_get_i64, extras_get_str, extras_get_u64};
use crate::media::MediaFormat;
use crate::media::formats::StreamFormat;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;

use regex::Regex;
use reqwest::Client;

use tracing::debug;

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?huya\.com/(\d+|[a-zA-Z0-9_-]+)").unwrap()
});

pub struct Huya {
    pub(super) extractor: Extractor,
    // whether to force the origin quality stream
    pub force_origin_quality: bool,
    pub api_mode: HuyaApiMode,
    pub platform: HuyaPlatform,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuyaApiMode {
    Wup,
    Mp,
    Web,
}

impl From<&str> for HuyaApiMode {
    fn from(value: &str) -> Self {
        match value {
            "WUP" => Self::Wup,
            "MP" => Self::Mp,
            "WEB" => Self::Web,
            _ => Self::Wup,
        }
    }
}

impl std::fmt::Display for HuyaApiMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wup => write!(f, "WUP"),
            Self::Mp => write!(f, "MP"),
            Self::Web => write!(f, "WEB"),
        }
    }
}

impl Huya {
    pub(super) const HUYA_URL: &'static str = "https://www.huya.com";

    pub fn new(
        platform_url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Huya", platform_url, client);
        extractor.set_origin_and_referer_static(Self::HUYA_URL);
        extractor.add_header_typed(reqwest::header::USER_AGENT, Self::get_wup_ua());
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        let force_origin_quality =
            extras_get_bool(extras.as_ref(), "force_origin_quality").unwrap_or(false);
        let api_mode = extras_get_str(extras.as_ref(), "api_mode").unwrap_or("WEB");
        let api_mode = HuyaApiMode::from(api_mode);
        let platform = extras_get_str(extras.as_ref(), "platform").unwrap_or("huya_pc_exe");
        let platform = HuyaPlatform::from(platform);
        Self {
            extractor,
            force_origin_quality,
            api_mode,
            platform,
        }
    }

    pub(super) fn force_origin_quality(&self, stream_name: &str) -> String {
        if self.force_origin_quality {
            // remove '-imgplus'
            stream_name.replace("-imgplus", "")
        } else {
            stream_name.to_string()
        }
    }

    /// Helper method to check HTTP response status and convert to ExtractorError if needed
    pub(super) async fn check_http_response(
        response: reqwest::Response,
    ) -> Result<reqwest::Response, ExtractorError> {
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(ExtractorError::HttpError(
                response.error_for_status().unwrap_err(),
            ));
        }
        Ok(response)
    }

    /// Helper method to create stream info for both FLV and HLS formats
    pub(super) fn create_stream_info(
        flv_url: &str,
        hls_url: &str,
        quality: &str,
        bitrate: u64,
        priority: u32,
        add_ratio: bool,
        extras: &serde_json::Value,
    ) -> Vec<StreamInfo> {
        vec![
            // FLV stream
            StreamInfo::builder(
                if add_ratio {
                    format!("{flv_url}&ratio={bitrate}")
                } else {
                    flv_url.to_string()
                },
                StreamFormat::Flv,
                MediaFormat::Flv,
            )
            .quality(quality)
            .bitrate(bitrate)
            .priority(priority)
            .codec("avc")
            .is_headers_needed(true)
            .extras(extras.clone())
            .build(),
            // HLS stream
            StreamInfo::builder(
                if add_ratio {
                    format!("{hls_url}&ratio={bitrate}")
                } else {
                    hls_url.to_string()
                },
                StreamFormat::Hls,
                MediaFormat::Ts,
            )
            .quality(quality)
            .bitrate(bitrate)
            .priority(priority)
            .codec("avc")
            .is_headers_needed(true)
            .extras(extras.clone())
            .build(),
        ]
    }

    /// Helper to build query with origin quality if needed
    pub(super) fn build_stream_query(
        &self,
        stream_name: &str,
        anti_code: &str,
        presenter_uid: i64,
    ) -> String {
        let stream_name = self.force_origin_quality(stream_name);
        get_anticode(
            &stream_name,
            anti_code,
            presenter_uid.try_into().ok(),
            self.platform,
        )
        .unwrap_or_else(|_| anti_code.to_string())
    }

    pub(super) fn extract_flv_url_from_extras(extras: &serde_json::Value) -> String {
        extras_get_str(Some(extras), "flv_url")
            .unwrap_or("")
            .to_owned()
    }

    /// Extract stream name from extras JSON
    pub(super) fn extract_stream_name_from_extras(
        extras: &serde_json::Value,
    ) -> Result<String, ExtractorError> {
        extras_get_str(Some(extras), "stream_name")
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                ExtractorError::ValidationError("Stream name not found in extras".to_string())
            })
    }

    /// Extract presenter UID from extras JSON
    pub(super) fn extract_presenter_uid_from_extras(extras: &serde_json::Value) -> i64 {
        extras_get_i64(Some(extras), "presenter_uid").unwrap_or(0)
    }

    /// Extract default bitrate from extras JSON
    pub(super) fn extract_default_bitrate_from_extras(extras: &serde_json::Value) -> u64 {
        extras_get_u64(Some(extras), "default_bitrate").unwrap_or(10000)
    }

    /// Map Huya's `iWebPriorityRate` (higher = better, 100 is a pin, -1 is
    /// "disabled for this ctype") to `StreamInfo.priority` (lower = better).
    /// Clamping makes the `-1` sentinel worst-but-valid — selection is left
    /// to `StreamSelector` (`preferred_cdns` / `blacklisted_cdns`) rather
    /// than filtered here, so users can opt in to a CDN Huya flagged as
    /// deprioritised if it works for them.
    pub(super) fn priority_from_web_rate(rate: i32) -> u32 {
        (100 - rate.clamp(0, 100)) as u32
    }

    pub(super) async fn get_room_page(&self) -> Result<String, ExtractorError> {
        let response = self.extractor.get(&self.extractor.url).send().await?;
        let response = Self::check_http_response(response).await?;
        let content = response.text().await?;
        Ok(content)
    }
}

#[async_trait]
impl PlatformExtractor for Huya {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let room_id = self
            .extractor
            .url
            .split('/')
            .next_back()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        // if we have a non-numeric room_id, or we are in WEB mode, use the web page
        if room_id == 0 || self.api_mode == HuyaApiMode::Web {
            let page_content = self.get_room_page().await?;
            return self.parse_web_media_info(&page_content).await;
        }

        match self.api_mode {
            HuyaApiMode::Mp => {
                let response_text = self.get_mp_page(room_id).await?;
                self.parse_mp_media_info(&response_text)
            }
            HuyaApiMode::Wup => {
                let (living_info, ua) = self.get_living_info_by_room_id_wup(room_id).await?;
                let presenter_uid = living_info.t_notice.l_presenter_uid;
                self.parse_living_info(&living_info, presenter_uid, &ua)
            }
            // SHOULD NEVER HAPPEN: WEB is already handled above
            HuyaApiMode::Web => unreachable!(),
        }
    }

    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        // MP based api requires no extra anticode computation
        if self.api_mode == HuyaApiMode::Mp && !self.force_origin_quality {
            return Ok(());
        }

        debug!("Getting WUP URL for stream: {}", stream_info.url);

        // Extract WUP request parameters from stream extras
        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Stream extras not found for WUP request".to_string())
        })?;

        // Compute anticode
        let flv_url = Self::extract_flv_url_from_extras(extras);
        let stream_name = Self::extract_stream_name_from_extras(extras)?;
        let presenter_uid = Self::extract_presenter_uid_from_extras(extras);
        let ua = crate::extractor::utils::extras_get_str(Some(extras), "ua")
            .unwrap_or("")
            .to_owned();
        self.get_anticode_url(stream_info, presenter_uid, &stream_name, &flv_url, &ua)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use crate::extractor::default::default_client;

    use super::*;

    fn read_test_file(file_name: &str) -> String {
        let mut d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("src/extractor/tests/test_data/huya/");
        d.push(file_name);
        std::fs::read_to_string(d).unwrap()
    }

    #[test]
    #[ignore]
    fn test_parse_mp_media_info() {
        let extractor = Huya::new(
            "https://www.huya.com/660000".to_string(),
            default_client(),
            None,
            None,
        );
        let response_str = read_test_file("mp_api_response.json");
        let media_info = extractor.parse_mp_media_info(&response_str).unwrap();

        assert!(media_info.is_live);
        assert_eq!(media_info.artist, "虎牙英雄联盟赛事");
        assert_eq!(media_info.title, "【预告】03点MKOI vs BLG MSI淘汰赛阶段");
        assert!(!media_info.streams.is_empty());
        assert_eq!(media_info.streams.len(), 12);
    }

    #[tokio::test]
    #[ignore]
    async fn test_is_live_integration() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
        let extractor = Huya::new(
            "https://www.huya.com/660000".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = extractor.extract().await;

        if let Err(e) = media_info {
            println!("{e}");
            return;
        }

        let mut media_info = media_info.unwrap();

        println!("{}", media_info.pretty_print());

        if !media_info.is_live {
            return;
        }
        // assert!(media_info.is_live);
        // println!("{media_info:?}");
        let mut stream_info = media_info.streams.drain(0..1).next().unwrap();
        assert!(!stream_info.url.is_empty());
        extractor.get_url(&mut stream_info).await.unwrap();

        println!("{}", stream_info.pretty_print(0, 200));
    }

    #[tokio::test]
    #[ignore]
    async fn test_mp_api() {
        let mut extractor = Huya::new(
            "https://www.huya.com/660000".to_string(),
            default_client(),
            None,
            None,
        );
        extractor.api_mode = HuyaApiMode::Mp;
        let media_info = extractor.extract().await.unwrap();
        assert!(media_info.is_live);
        assert!(!media_info.streams.is_empty());
        println!("{media_info:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_decode_wup_response() {
        let response_bytes = std::fs::read("D:/Develop/hua0512/rust-srec/wup_response.bin")
            .unwrap()
            .into();
        let token_info =
            crate::extractor::platforms::huya::tars::decode_get_cdn_token_info_response(
                response_bytes,
            )
            .unwrap();
        println!("{token_info:?}");
    }
}
