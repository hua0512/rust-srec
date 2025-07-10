use std::sync::LazyLock;

use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::huya::huya_tars::decode_get_cdn_token_info_response;
use crate::media::MediaFormat;
use crate::media::formats::StreamFormat;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use url::Url;

use super::huya_tars;
use super::models::*;

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?huya\.com/(\d+|[a-zA-Z0-9_-]+)").unwrap()
});

static ROOM_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var TT_ROOM_DATA = (.*?);"#).unwrap());

static PROFILE_INFO_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var TT_PROFILE_INFO = (.*?);"#).unwrap());
static STREAM_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"stream: (\{.+)\n.*?};"#).unwrap());

pub struct HuyaExtractor {
    extractor: Extractor,
    // whether to use WUP (Web Unicast Protocol) for extraction
    // if not set, the extractor will use the MP api for extraction
    pub use_wup: bool,
    // whether to force the origin quality stream
    pub force_origin_quality: bool,
    pub extras: Option<serde_json::Value>,
}

impl HuyaExtractor {
    const HUYA_URL: &'static str = "https://www.huya.com";
    const WUP_URL: &'static str = "https://wup.huya.com";
    const MP_URL: &'static str = "https://mp.huya.com/cache.php";
    // WUP User-Agent for Huya
    const WUP_UA: &'static str =
        "HYSDK(Windows, 30000002)_APP(pc_exe&6090003&official)_SDK(trans&2.24.0.5030)";

    pub fn new(
        platform_url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Huya".to_string(), platform_url, client);
        let huya_url = Self::HUYA_URL.to_string();
        extractor.add_header(reqwest::header::ORIGIN.to_string(), huya_url.clone());
        extractor.add_header(reqwest::header::REFERER.to_string(), huya_url);
        extractor.add_header(
            reqwest::header::USER_AGENT.to_string(),
            Self::WUP_UA.to_string(),
        );
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        Self {
            extractor,
            use_wup: true,
            force_origin_quality: true,
            extras,
        }
    }

    fn force_origin_quality(&self, stream_name: &str) -> String {
        if self.force_origin_quality {
            // remove '-imgplus'
            stream_name.replace("-imgplus", "")
        } else {
            stream_name.to_string()
        }
    }

    async fn get_room_page(&self) -> Result<String, ExtractorError> {
        let response = self.extractor.get(&self.extractor.url).send().await?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(ExtractorError::HttpError(
                response.error_for_status().unwrap_err(),
            ));
        }
        let content = response.text().await?;
        Ok(content)
    }

    async fn get_mp_page(&self, room_id: i64) -> Result<String, ExtractorError> {
        let url = format!(
            "{}?do=profileRoom&m=Live&roomid={}&showSecret=1",
            Self::MP_URL,
            room_id
        );
        let response = self.extractor.get(&url).send().await?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(ExtractorError::HttpError(
                response.error_for_status().unwrap_err(),
            ));
        }
        let content = response.text().await?;
        Ok(content)
    }

    pub(crate) fn parse_mp_live_status(
        &self,
        response: &MpApiResponse,
    ) -> Result<bool, ExtractorError> {
        if response.status != 200 {
            if response.status == 422
                && (response.message.contains("主播不存在")
                    || response.message.contains("该主播不存在"))
            {
                return Err(ExtractorError::StreamerNotFound);
            }
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get live status: status {}, message: {}",
                response.status, response.message
            )));
        }

        let data = match &response.data {
            Some(data) => data,
            None => return Err(ExtractorError::StreamerNotFound),
        };

        if let Some(live_data) = &data.live_data {
            if live_data.introduction.starts_with("【回放】") {
                return Ok(false);
            }
        }

        let is_live = data.real_live_status == Some("ON") && data.live_status == Some("ON");

        Ok(is_live)
    }

    pub(crate) fn parse_mp_media_info<'a>(
        &self,
        response_text: &'a str,
    ) -> Result<MediaInfo, ExtractorError> {
        let response: MpApiResponse<'a> =
            serde_json::from_str(response_text).map_err(ExtractorError::JsonError)?;

        if response.status != 200 {
            if response.status == 422
                && (response.message.contains("主播不存在")
                    || response.message.contains("该主播不存在"))
            {
                return Err(ExtractorError::StreamerNotFound);
            }
            return Err(ExtractorError::ValidationError(format!(
                "API error: status {}, message: {}",
                response.status, response.message
            )));
        }

        let data = match &response.data {
            Some(data) => data,
            None => return Err(ExtractorError::StreamerNotFound),
        };

        let profile_info = match &data.profile_info {
            Some(info) => info,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No profile info found".to_string(),
                ));
            }
        };

        let presenter_uid = profile_info.uid;
        let avatar_url = Some(profile_info.avatar180.to_string());
        let artist = profile_info.nick.to_string();

        let live_data = match &data.live_data {
            Some(data) => data,
            None => {
                return Ok(MediaInfo::new(
                    self.extractor.url.clone(),
                    "".to_string(),
                    artist,
                    None,
                    avatar_url,
                    false,
                    vec![],
                    None,
                ));
            }
        };

        let title = live_data.introduction.to_string();
        let cover_url = Some(live_data.screenshot.to_string());

        let is_live = self.parse_mp_live_status(&response)?;

        if !is_live {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                title,
                artist,
                cover_url,
                avatar_url,
                false,
                vec![],
                None,
            ));
        }

        let stream_data = match &data.stream {
            Some(data) => data,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No stream data found".to_string(),
                ));
            }
        };

        let stream_info_list = &stream_data.base_steam_info_list;
        let bitrate_info_list = &stream_data.bit_rate_info;
        let default_bitrate = live_data.bit_rate as u64;

        let streams = self.parse_streams(
            stream_info_list,
            bitrate_info_list,
            default_bitrate,
            presenter_uid,
        )?;

        Ok(MediaInfo::new(
            self.extractor.url.clone(),
            title,
            artist,
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
        ))
    }

    pub(crate) fn parse_live_status(&self, response_text: &str) -> Result<bool, ExtractorError> {
        if response_text.contains("找不到这个主播") {
            return Err(ExtractorError::StreamerNotFound);
        }

        if response_text.contains("该主播涉嫌违规，正在整改中") {
            return Err(ExtractorError::StreamerBanned);
        }

        if response_text.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ));
        }

        let room_data_str = ROOM_DATA_REGEX
            .captures(response_text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ))?;

        let room_data: RoomData =
            serde_json::from_str(room_data_str).map_err(ExtractorError::JsonError)?;

        if room_data.introduction.contains("【回放】") {
            return Ok(false);
        }

        if room_data.state != "ON" {
            return Ok(false);
        }

        Ok(true)
    }

    pub(crate) fn parse_web_media_info(
        &self,
        page_content: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        let live_status = self.parse_live_status(page_content)?;

        let profile_info_str = PROFILE_INFO_REGEX
            .captures(page_content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find profile info".to_string())
            })?;

        let profile_info: WebProfileInfo =
            serde_json::from_str(profile_info_str).map_err(ExtractorError::JsonError)?;

        let artist = profile_info.nick;

        if profile_info.lp <= 0 {
            return Err(ExtractorError::StreamerNotFound);
        }

        let avatar_url = if profile_info.avatar.is_empty() {
            None
        } else {
            Some(profile_info.avatar.to_string())
        };

        if !live_status {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                "直播未开始".to_string(),
                artist.to_string(),
                None,
                avatar_url,
                false,
                vec![],
                None,
            ));
        }

        let stream_data_str = STREAM_DATA_REGEX
            .captures(page_content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find stream object".to_string())
            })?;

        let stream_response: WebStreamResponse =
            serde_json::from_str(stream_data_str).map_err(ExtractorError::JsonError)?;

        let stream_container = stream_response.data.first().ok_or_else(|| {
            ExtractorError::ValidationError("No stream data container found".to_string())
        })?;

        let game_live_info = &stream_container.game_live_info;

        let presenter_uid = game_live_info.uid;
        let title = &game_live_info.room_name;
        let cover_url = if game_live_info.screenshot.is_empty() {
            None
        } else {
            Some(game_live_info.screenshot.to_string())
        };

        let stream_info_list = &stream_container.game_stream_info_list;
        let default_bitrate = game_live_info.bit_rate as u64;
        let bitrate_info_list = &stream_response.v_multi_stream_info;

        let streams = self.parse_streams(
            stream_info_list,
            bitrate_info_list,
            default_bitrate,
            presenter_uid,
        )?;

        Ok(MediaInfo::new(
            self.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            true,
            streams,
            Some(self.extractor.get_platform_headers_map()),
        ))
    }

    pub(crate) fn parse_streams(
        &self,
        stream_info_list: &[StreamInfoItem],
        bitrate_info_list: &[BitrateInfo],
        default_bitrate: u64,
        presenter_uid: i64,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut streams = Vec::new();
        for stream_info in stream_info_list.iter() {
            if stream_info.s_stream_name.is_empty() {
                continue;
            }

            let stream_name = self.force_origin_quality(stream_info.s_stream_name);

            let flv_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_flv_url,
                stream_name,
                stream_info.s_flv_url_suffix,
                stream_info.s_flv_anti_code
            );

            let hls_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_hls_url,
                stream_name,
                stream_info.s_hls_url_suffix,
                stream_info.s_hls_anti_code
            );

            let extras = serde_json::json!({
                "cdn": stream_info.s_cdn_type,
                "stream_name": stream_name,
                "presenter_uid": presenter_uid,
                "default_bitrate": default_bitrate,
            });

            let add_streams_for_bitrate =
                |streams: &mut Vec<StreamInfo>,
                 quality: &str,
                 bitrate: u64,
                 priority: u32,
                 extras: &serde_json::Value| {
                    // flv
                    streams.push(StreamInfo {
                        url: format!("{flv_url}&ratio={bitrate}"),
                        stream_format: StreamFormat::Flv,
                        media_format: MediaFormat::Flv,
                        quality: quality.to_string(),
                        bitrate,
                        priority,
                        codec: "avc".to_string(),
                        is_headers_needed: false,
                        fps: 0.0,
                        extras: Some(extras.clone()),
                    });
                    // hls
                    streams.push(StreamInfo {
                        url: format!("{hls_url}&ratio={bitrate}"),
                        stream_format: StreamFormat::Hls,
                        media_format: MediaFormat::Ts,
                        quality: quality.to_string(),
                        bitrate,
                        priority,
                        codec: "avc".to_string(),
                        is_headers_needed: false,
                        fps: 0.0,
                        extras: Some(extras.clone()),
                    });
                };

            let priority = stream_info.i_web_priority_rate as u32;

            if bitrate_info_list.is_empty() {
                add_streams_for_bitrate(&mut streams, "原画", default_bitrate, priority, &extras);
            } else {
                for bitrate_info in bitrate_info_list.iter() {
                    if bitrate_info.s_display_name.contains("HDR") {
                        continue;
                    }
                    add_streams_for_bitrate(
                        &mut streams,
                        bitrate_info.s_display_name.as_ref(),
                        bitrate_info.i_bit_rate.into(),
                        priority,
                        &extras,
                    );
                }
            }
        }

        Ok(streams)
    }

    async fn get_stream_url_wup(
        &self,
        stream_info: &mut StreamInfo,
        cdn: &str,
        stream_name: &str,
        presenter_uid: i32,
    ) -> Result<(), ExtractorError> {
        // println!("Getting true url for {:?}", stream_info);
        let request_body = huya_tars::build_get_cdn_token_info_request(
            stream_name.to_string(),
            cdn.to_string(),
            presenter_uid,
        )
        .unwrap();

        let response = self
            .extractor
            .post(Self::WUP_URL)
            .body(request_body)
            .send()
            .await?;

        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(ExtractorError::HttpError(
                response.error_for_status().unwrap_err(),
            ));
        }

        let response_bytes = response.bytes().await?;

        let token_info = decode_get_cdn_token_info_response(response_bytes)
            .expect("Failed to decode WUP response");

        // query params
        let anti_code = match stream_info.stream_format {
            StreamFormat::Flv => token_info.flv_anti_code,
            StreamFormat::Hls => token_info.hls_anti_code,
            _ => {
                return Err(ExtractorError::ValidationError(format!(
                    "Invalid stream format: {:?}",
                    stream_info.stream_format
                )));
            }
        };

        let s_stream_name = stream_name;

        let url = Url::parse(&stream_info.url).unwrap();
        let host = url.host_str().unwrap_or("");
        let path = url.path().split('/').nth(1).unwrap_or("");
        let base_url = format!("{}://{}/{}", url.scheme(), host, path);
        // println!("Base URL: {:?}", base_url);

        let suffix = match stream_info.stream_format {
            StreamFormat::Flv => "flv",
            StreamFormat::Hls => "m3u8",
            _ => {
                return Err(ExtractorError::ValidationError(format!(
                    "Invalid stream format: {:?}",
                    stream_info.stream_format
                )));
            }
        };

        let bitrate = stream_info.bitrate;

        // use match closure
        let default_bitrate = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("default_bitrate"))
            .and_then(|v| v.as_u64())
            .unwrap_or(10000);

        // Use reqwest's Url for safe query parameter handling
        let base_url = format!("{base_url}/{s_stream_name}.{suffix}?{anti_code}");

        if bitrate != default_bitrate {
            let new_url = format!("{base_url}&ratio={bitrate}");
            stream_info.url = new_url.to_string();
        } else {
            stream_info.url = base_url.to_string();
        }

        Ok(())
    }
}

#[async_trait]
impl PlatformExtractor for HuyaExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        // use MP API
        if !self.use_wup {
            let room_id = self
                .extractor
                .url
                .split('/')
                .next_back()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| {
                    ExtractorError::InvalidUrl("Huya MP API requires numeric room ID".to_string())
                })?;
            let response_text = self.get_mp_page(room_id).await?;
            let media_info = self.parse_mp_media_info(&response_text)?;
            return Ok(media_info);
        }

        // use web api
        let page_content = self.get_room_page().await?;
        let media_info = self.parse_web_media_info(&page_content)?;
        return Ok(media_info);
    }

    async fn get_url(&self, mut stream_info: StreamInfo) -> Result<StreamInfo, ExtractorError> {
        // if not wup, return the stream info directly
        if !self.use_wup {
            return Ok(stream_info);
        }

        // wup method
        let extras = stream_info
            .extras
            .as_ref()
            .ok_or_else(|| {
                ExtractorError::ValidationError(
                    "Stream extras not found for WUP request".to_string(),
                )
            })
            .cloned()
            .unwrap();

        let cdn = extras.get("cdn").and_then(|v| v.as_str()).unwrap_or("AL");

        let stream_name = extras
            .get("stream_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Stream name not found in extras".to_string())
            })?;

        let presenter_uid = extras
            .get("presenter_uid")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .unwrap_or(0);

        self.get_stream_url_wup(&mut stream_info, cdn, stream_name, presenter_uid)
            .await?;

        Ok(stream_info)
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
    fn test_parse_mp_live_status() {
        let extractor = HuyaExtractor::new(
            "https://www.huya.com/".to_string(),
            default_client(),
            None,
            None,
        );

        let response_str = read_test_file("mp_api_response.json");
        let mut response: MpApiResponse = serde_json::from_str(&response_str).unwrap();

        // Test case 1: Live on
        response.data.as_mut().unwrap().real_live_status = Some("ON");
        response.data.as_mut().unwrap().live_status = Some("ON");
        assert!(extractor.parse_mp_live_status(&response).unwrap());

        // Test case 2: Live off
        response.data.as_mut().unwrap().real_live_status = Some("OFF");
        assert!(!extractor.parse_mp_live_status(&response).unwrap());

        // Test case 3: Replay
        response.data.as_mut().unwrap().real_live_status = Some("ON");
        response.data.as_mut().unwrap().live_status = Some("ON");
        response
            .data
            .as_mut()
            .unwrap()
            .live_data
            .as_mut()
            .unwrap()
            .introduction = "【回放】".to_string().into();
        assert!(!extractor.parse_mp_live_status(&response).unwrap());

        // Test case 4: Streamer not found
        response.status = 422;
        response.message = "主播不存在";
        let result = extractor.parse_mp_live_status(&response);
        assert!(matches!(result, Err(ExtractorError::StreamerNotFound)));
    }

    #[test]
    fn test_parse_mp_media_info() {
        let extractor = HuyaExtractor::new(
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
        let extractor = HuyaExtractor::new(
            "https://www.huya.com/660000".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = extractor.extract().await.unwrap();
        assert!(media_info.is_live);
        let stream_info = media_info.streams.first().unwrap();
        assert!(!stream_info.url.is_empty());

        let stream_info = extractor.get_url(stream_info.clone()).await.unwrap();

        println!("{stream_info:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_mp_api() {
        let mut extractor = HuyaExtractor::new(
            "https://www.huya.com/660000".to_string(),
            default_client(),
            None,
            None,
        );
        extractor.use_wup = false;
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
        let token_info = decode_get_cdn_token_info_response(response_bytes).unwrap();
        println!("{token_info:?}");
    }
}
