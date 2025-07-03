use std::collections::HashMap;

use crate::extractor::error::ExtractorError;
use crate::extractor::extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::huya::huya_tars::decode_get_cdn_token_info_response;
use crate::media::media_format::MediaFormat;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use url::Url;

use super::huya_tars;

static ROOM_DATA_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"var TT_ROOM_DATA = (.*?);"#).unwrap());

static PROFILE_INFO_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"var TT_PROFILE_INFO = (.*?);"#).unwrap());
static STREAM_DATA_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"stream: (\{.+)\n.*?};"#).unwrap());

pub struct HuyaExtractor {
    extractor: Extractor,
    // whether to use WUP (Web Unicast Protocol) for extraction
    // if not set, the extractor will use the MP api for extraction
    pub use_wup: bool,
}

impl HuyaExtractor {
    const HUYA_URL: &'static str = "https://www.huya.com";
    const WUP_URL: &'static str = "https://wup.huya.com";
    const MP_URL: &'static str = "https://mp.huya.com/cache.php";
    // WUP User-Agent for Huya
    const WUP_UA: &'static str =
        "HYSDK(Windows, 30000002)_APP(pc_exe&6090003&official)_SDK(trans&2.24.0.5030)";

    pub fn new(platform_url: String, client: Client) -> Self {
        let mut extractor = Extractor::new("Huya".to_string(), platform_url, client);
        let huya_url = Self::HUYA_URL.to_string();
        extractor.add_header("Origin".to_string(), huya_url.clone());
        extractor.add_header("Referer".to_string(), huya_url);
        extractor.add_header("User-Agent".to_string(), Self::WUP_UA.to_string());
        Self {
            extractor,
            use_wup: true,
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
            "{}?m=Live&do=profileRoom&roomId={}&showSecret=1",
            Self::MP_URL,
            room_id
        );
        let response = self.extractor.get(&url).send().await?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(ExtractorError::HttpError(
                response.error_for_status().unwrap_err(),
            ));
        }
        let content = response.json().await?;
        Ok(content)
    }

    pub(crate) fn parse_mp_live_status(
        &self,
        json: &serde_json::Value,
    ) -> Result<bool, ExtractorError> {
        let status = match json.get("status") {
            Some(data) => data.as_i64(),
            None => return Err(ExtractorError::ValidationError("No data found".to_string())),
        };
        let messages = match json.get("message") {
            Some(data) => data.as_str(),
            None => Some(""),
        };

        // status is present
        let status = status.unwrap() as i32;

        if status != 200 {
            // streamer not found
            if status == 422 && messages.is_some() && messages.unwrap().contains("主播不存在")
            {
                return Err(ExtractorError::StreamerNotFound);
            }
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get live status: status {}, message: {}",
                status,
                messages.unwrap_or("No message provided").to_string()
            )));
        }

        // status is 200

        let data_json = match json.get("data") {
            Some(data) => data,
            None => return Err(ExtractorError::ValidationError("No data found".to_string())),
        };

        let real_room_status = match data_json.get("realLiveStatus") {
            Some(data) => data.as_str(),
            None => Some("OFF"),
        };
        let live_status = match data_json.get("liveStatus") {
            Some(data) => data.as_str(),
            None => Some("OFF"),
        };

        let live_data_json = data_json.get("liveData");

        if let Some(live_data) = live_data_json {
            let intro = live_data
                .get("introduction")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if intro.starts_with("【回放】") {
                return Ok(false);
            }
        }

        let is_live = real_room_status == Some("ON") && live_status == Some("ON");

        Ok(is_live)
    }

    pub(crate) fn parse_mp_media_info(
        &self,
        json: &serde_json::Value,
    ) -> Result<MediaInfo, ExtractorError> {
        let data = match json.get("data") {
            Some(data) => data,
            None => return Err(ExtractorError::ValidationError("No data found".to_string())),
        };

        let profile_info = match data.get("profileInfo") {
            Some(info) => info,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No profileInfo found".to_string(),
                ));
            }
        };

        let presenter_uid = match profile_info.get("uid") {
            Some(data) => data.as_i64(),
            None => {
                return Err(ExtractorError::ValidationError(
                    "No presenter UID found".to_string(),
                ));
            }
        };

        let avatar_url = profile_info
            .get("avatar180")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let artist = profile_info
            .get("nick")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let live_data = match data.get("liveData") {
            Some(data) => data,
            // when no livedata is found, live status is false or livestatus is frozen
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

        let title = live_data
            .get("introduction")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let cover_url = live_data
            .get("screenshot")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let is_live = self.parse_mp_live_status(json)?;

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

        let stream_json = match data.get("stream") {
            Some(data) => data,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No stream data found".to_string(),
                ));
            }
        };

        // stream json can be empty or null, which means no streams are available
        if stream_json.is_null()
            || stream_json.is_array() && stream_json.as_array().unwrap().is_empty()
            || !stream_json.is_object()
        {
            return Err(ExtractorError::NoStreamsFound);
        }

        let stream_info_list = match stream_json.get("baseSteamInfoList") {
            Some(data) => data.as_array(),
            None => {
                return Err(ExtractorError::ValidationError(
                    "No baseSteamInfoList found".to_string(),
                ));
            }
        };

        let stream_info_list = match stream_info_list {
            Some(list) => list,
            None => {
                return Err(ExtractorError::ValidationError(
                    "baseSteamInfoList is not an array".to_string(),
                ));
            }
        };

        let bitrate_info_array = match stream_json.get("bitRateInfo") {
            Some(data) => data.as_array(),
            None => stream_json
                .get("flv")
                .and_then(|v| v.get("rateArray").and_then(|v| v.as_array()))
                .or_else(|| {
                    stream_json
                        .get("hls")
                        .and_then(|v| v.get("rateArray").and_then(|v| v.as_array()))
                }),
        };

        let empty_vec = vec![];
        let bitrate_info_array = bitrate_info_array.unwrap_or(&empty_vec);

        let default_bitrate = live_data
            .get("bitRate")
            .and_then(|v| v.as_u64())
            .unwrap_or(10000) as u32;

        let presenter_uid = presenter_uid.unwrap();

        let streams = self.parse_streams(
            stream_info_list,
            bitrate_info_array,
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
            Some(self.get_platform_headers().clone()),
        ))
    }

    pub(crate) fn parse_live_status(&self, response_text: &str) -> Result<bool, ExtractorError> {
        if response_text.contains("找不到这个主播") {
            return Err(ExtractorError::StreamerNotFound);
        }

        if response_text.contains("该主播涉嫌违规，正在整改中") {
            return Err(ExtractorError::StreamerBanned);
        }

        let room_data = ROOM_DATA_REGEX
            .captures(response_text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ))?;

        let room_data_json: serde_json::Value =
            serde_json::from_str(room_data).map_err(|e| ExtractorError::JsonError(e))?;

        let intro = match room_data_json.get("introduction") {
            Some(v) => v.as_str().unwrap_or(""),
            None => {
                return Err(ExtractorError::ValidationError(
                    "Introduction not found".to_string(),
                ));
            }
        };

        let state = match room_data_json.get("state") {
            Some(v) => v.as_str().unwrap_or(""),
            None => {
                return Err(ExtractorError::ValidationError(
                    "State not found".to_string(),
                ));
            }
        };

        if intro.contains("【回放】") {
            return Ok(false);
        }

        if state != "ON" {
            return Ok(false);
        }

        Ok(true)
    }

    pub(crate) fn parse_web_media_info(
        &self,
        page_content: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        let live_status = self.parse_live_status(&page_content)?;

        let profile_info = PROFILE_INFO_REGEX
            .captures(&page_content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find profile info".to_string())
            })?;

        let profile_info_json: serde_json::Value =
            serde_json::from_str(profile_info).map_err(|e| ExtractorError::JsonError(e))?;

        let nick = profile_info_json["nick"].as_str().unwrap_or("").to_string();

        // check if presenter uid is present
        profile_info_json["lp"]
            .as_i64()
            .filter(|&uid| uid > 0)
            .ok_or(ExtractorError::StreamerNotFound)?;

        let avatar_url = profile_info_json["avatar"].as_str().map(|s| s.to_string());

        if !live_status {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                "直播未开始".to_string(),
                nick.to_string(),
                None,
                avatar_url,
                false,
                vec![],
                None,
            ));
        }

        let stream_data = STREAM_DATA_REGEX
            .captures(&page_content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find stream object".to_string())
            })?;

        let config_json: serde_json::Value =
            serde_json::from_str(&stream_data).map_err(|e| ExtractorError::JsonError(e))?;

        let game_live_info = &config_json["data"][0]["gameLiveInfo"];
        let presenter_uid = game_live_info["uid"].as_i64().ok_or_else(|| {
            ExtractorError::ValidationError("Presenter UID not found".to_string())
        })?;
        let title = game_live_info["roomName"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let artist = game_live_info["nick"].as_str().unwrap_or("").to_string();
        let cover_url = game_live_info["screenshot"].as_str().map(|s| s.to_string());

        let stream_info_list = config_json["data"][0]["gameStreamInfoList"]
            .as_array()
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find gameStreamInfoList".to_string())
            })?;

        let default_bitrate = game_live_info["bitRate"].as_u64().unwrap_or(10000) as u32;

        let empty_vec = vec![];
        let bitrate_info_list = config_json["vMultiStreamInfo"]
            .as_array()
            .unwrap_or(&empty_vec);

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
            true,
            streams,
            Some(self.get_platform_headers().clone()),
        ))
    }

    pub(crate) fn parse_streams(
        &self,
        stream_info_list: &[serde_json::Value],
        bitrate_info_list: &[serde_json::Value],
        default_bitrate: u32,
        presenter_uid: i64,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut streams = Vec::new();
        for stream_info_json in stream_info_list.iter() {
            let s_stream_name = stream_info_json["sStreamName"].as_str().unwrap_or("");
            let s_flv_url = stream_info_json["sFlvUrl"].as_str().unwrap_or("");
            let s_flv_url_suffix = stream_info_json["sFlvUrlSuffix"].as_str().unwrap_or("");
            let s_flv_anti_code = stream_info_json["sFlvAntiCode"].as_str().unwrap_or("");
            let s_cdn_type = stream_info_json["sCdnType"].as_str().unwrap_or("");

            let s_hls_url = stream_info_json["sHlsUrl"].as_str().unwrap_or("");
            let s_hls_url_suffix = stream_info_json["sHlsUrlSuffix"].as_str().unwrap_or("");
            let s_hls_anti_code = stream_info_json["sHlsAntiCode"].as_str().unwrap_or("");

            if s_stream_name.is_empty() {
                continue;
            }

            let flv_url = format!(
                "{}/{}.{}?{}",
                s_flv_url, s_stream_name, s_flv_url_suffix, s_flv_anti_code
            );

            let hls_url = format!(
                "{}/{}.{}?{}",
                s_hls_url, s_stream_name, s_hls_url_suffix, s_hls_anti_code
            );

            let mut extras = HashMap::new();
            extras.insert("cdn".to_string(), s_cdn_type.to_string());
            extras.insert("stream_name".to_string(), s_stream_name.to_string());
            extras.insert("presenter_uid".to_string(), presenter_uid.to_string());
            extras.insert("default_bitrate".to_string(), default_bitrate.to_string());

            let add_streams_for_bitrate =
                |streams: &mut Vec<StreamInfo>,
                 quality: String,
                 bitrate: u32,
                 priority: u32,
                 extras: &HashMap<String, String>| {
                    // flv
                    streams.push(StreamInfo {
                        url: flv_url.clone() + &format!("?ratio={}", bitrate),
                        format: MediaFormat::Flv,
                        quality: quality.clone(),
                        bitrate,
                        priority,
                        codec: "avc".to_string(),
                        is_headers_needed: false,
                        extras: Some(extras.clone()),
                    });
                    // hls
                    streams.push(StreamInfo {
                        url: hls_url.clone() + &format!("?ratio={}", bitrate),
                        format: MediaFormat::Hls,
                        quality,
                        bitrate,
                        priority,
                        codec: "avc".to_string(),
                        is_headers_needed: false,
                        extras: Some(extras.clone()),
                    });
                };

            let priority = stream_info_json["iWebPriorityRate"].as_u64().unwrap_or(0) as u32;

            if bitrate_info_list.is_empty() {
                add_streams_for_bitrate(
                    &mut streams,
                    "原画".to_string(),
                    default_bitrate,
                    priority,
                    &extras,
                );
            } else {
                for bitrate_info in bitrate_info_list.iter() {
                    let s_display_name = bitrate_info["sDisplayName"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    // HDR is not supported
                    if s_display_name.contains("HDR") {
                        continue;
                    }
                    let s_bitrate = bitrate_info["iBitRate"].as_u64().unwrap_or(0) as u32;
                    add_streams_for_bitrate(
                        &mut streams,
                        s_display_name,
                        s_bitrate,
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
        let anti_code = match stream_info.format {
            MediaFormat::Flv => token_info.flv_anti_code,
            MediaFormat::Hls => token_info.hls_anti_code,
        };

        let s_stream_name = stream_name;

        let url = Url::parse(&stream_info.url).unwrap();
        let host = url.host_str().unwrap_or("");
        let path = url.path().split('/').nth(1).unwrap_or("");
        let base_url = format!("{}://{}/{}", url.scheme(), host, path);
        // println!("Base URL: {:?}", base_url);

        let suffix = match stream_info.format {
            MediaFormat::Flv => "flv",
            MediaFormat::Hls => "m3u8",
        };

        let bitrate = stream_info.bitrate;

        // use match closure
        let default_bitrate = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("default_bitrate"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10000);

        // Use reqwest's Url for safe query parameter handling
        let base_url = format!("{}/{}.{}?{}", base_url, s_stream_name, suffix, anti_code);

        if bitrate != default_bitrate {
            let new_url = format!("{}&ratio={}", base_url, bitrate);
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
                .last()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| {
                    ExtractorError::InvalidUrl("Huya MP API requires numeric room ID".to_string())
                })?;
            let json = self.get_mp_page(room_id).await?;
            let json_value = serde_json::from_str::<serde_json::Value>(&json)
                .map_err(|e| ExtractorError::JsonError(e))?;
            let media_info = self.parse_mp_media_info(&json_value)?;
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
        let cdn = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("cdn"))
            .map(|s| s.as_str())
            .unwrap_or("AL")
            .to_string();

        let stream_name = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("stream_name"))
            .cloned()
            .ok_or_else(|| ExtractorError::ValidationError("Stream name not found".to_string()))?;

        let presenter_uid = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("presenter_uid"))
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        // println!(
        //     "Using WUP to get stream URL: CDN: {}, Stream Name: {}, Presenter UID: {}",
        //     cdn, stream_name, presenter_uid
        // );

        self.get_stream_url_wup(&mut stream_info, &cdn, &stream_name, presenter_uid)
            .await?;

        Ok(stream_info)
    }
}

#[cfg(test)]
mod tests {

    use crate::extractor::default::default_client;

    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_is_live_integration() {
        let extractor =
            HuyaExtractor::new("https://www.huya.com/660000".to_string(), default_client());
        let media_info = extractor.extract().await.unwrap();
        assert_eq!(media_info.is_live, true);
        let stream_info = media_info.streams.first().unwrap();
        assert!(!stream_info.url.is_empty());

        let stream_info = extractor.get_url(stream_info.clone()).await.unwrap();

        println!("{:?}", stream_info);
    }

    #[tokio::test]
    #[ignore]
    async fn test_mp_api() {
        let extractor =
            HuyaExtractor::new("https://www.huya.com/660000".to_string(), default_client());
        let media_info = extractor.extract().await.unwrap();
        assert_eq!(media_info.is_live, true);
        assert!(!media_info.streams.is_empty());
        println!("{:?}", media_info);
    }

    #[tokio::test]
    #[ignore]
    async fn test_decode_wup_response() {
        let response_bytes = std::fs::read("D:/Develop/hua0512/rust-srec/wup_response.bin")
            .unwrap()
            .into();
        let token_info = decode_get_cdn_token_info_response(response_bytes).unwrap();
        println!("{:?}", token_info);
    }
}
