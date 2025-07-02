use std::collections::HashMap;

use crate::extractor::error::ExtractorError;
use crate::extractor::extractor::{Extractor, PlatformExtractor};
use crate::media::media_format::MediaFormat;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use tars_codec::types::TarsValue;

use super::huya_tars;

static ROOM_DATA_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"var TT_ROOM_DATA = (.*?);"#).unwrap());
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
        extractor.add_param("User-Agent".to_string(), Self::WUP_UA.to_string());
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
            .ok_or(ExtractorError::Other(
                "Failed to extract room data".to_string(),
            ))?;

        let room_data_json: serde_json::Value =
            serde_json::from_str(room_data).map_err(|e| ExtractorError::JsonError(e))?;

        let intro = match room_data_json.get("introduction") {
            Some(v) => v.as_str().unwrap_or(""),
            None => return Err(ExtractorError::Other("Introduction not found".to_string())),
        };

        let state = match room_data_json.get("state") {
            Some(v) => v.as_str().unwrap_or(""),
            None => return Err(ExtractorError::Other("State not found".to_string())),
        };

        if intro.contains("【回放】") {
            return Ok(false);
        }

        if state != "ON" {
            return Ok(false);
        }

        Ok(true)
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
        let request_body = huya_tars::build_get_cdn_token_info_request(
            stream_name.to_string(),
            cdn.to_string(),
            presenter_uid,
        )
        .unwrap();

        println!("WUP Request Body: {:?}", request_body);
       return Ok(());

        let response = self
            .extractor
            .post(Self::WUP_URL)
            .body(request_body)
            .send()
            .await?;

        let response_bytes = response.bytes().await?;

        println!("WUP Response: {:?}", response_bytes);

        let decoded_response = tars_codec::decode_response(&mut response_bytes.into()).unwrap();

        if let Some(response_message) = decoded_response {
            if let Some(body_bytes) = response_message.body.get("tRsp") {
                let tars_value: TarsValue = tars_codec::de::from_bytes(body_bytes);
                if let Ok(resp) = huya_tars::HuyaGetTokenResp::try_from(tars_value) {
                    println!(
                        "CDN: {}, Stream Name: {}, FLV Anti Code: {}",
                        resp.cdn_type, resp.stream_name, resp.flv_anti_code
                    );

                    let s_flv_anti_code = resp.flv_anti_code;
                    let s_stream_name = stream_name;
                    let s_cdn_type = resp.cdn_type;
                    let s_flv_url = format!("https://{}.flv.huya.com/src", s_cdn_type);
                    let s_flv_url_suffix = "flv";

                    let new_url = format!(
                        "{}/{}.{}?{}",
                        s_flv_url, s_stream_name, s_flv_url_suffix, s_flv_anti_code
                    );
                    stream_info.url = new_url;
                    return Ok(());
                }
            }
        }

        Err(ExtractorError::Other(
            "Failed to get stream url from wup".to_string(),
        ))
    }
}

#[async_trait]
impl PlatformExtractor for HuyaExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let page_content = self.get_room_page().await?;
        let live_status = self.parse_live_status(&page_content)?;

        if !live_status {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                "直播未开始".to_string(),
                "未知".to_string(),
                None,
                None,
                false,
                vec![],
                None,
            ));
        }

        let stream_data = STREAM_DATA_REGEX
            .captures(&page_content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::Other("Could not find stream object".to_string()))?;

        let config_json: serde_json::Value =
            serde_json::from_str(&stream_data).map_err(|e| ExtractorError::JsonError(e))?;

        let game_live_info = &config_json["data"][0]["gameLiveInfo"];
        let presenter_uid = game_live_info["uid"]
            .as_i64()
            .ok_or_else(|| ExtractorError::Other("Presenter UID not found".to_string()))?;
        let title = game_live_info["roomName"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let artist = game_live_info["nick"].as_str().unwrap_or("").to_string();
        let cover_url = game_live_info["screenshot"].as_str().map(|s| s.to_string());

        let stream_info_list = config_json["data"][0]["gameStreamInfoList"]
            .as_array()
            .ok_or_else(|| {
                ExtractorError::Other("Could not find gameStreamInfoList".to_string())
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
            None,
            true,
            streams,
            None,
        ))
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
            .ok_or_else(|| ExtractorError::Other("Stream name not found".to_string()))?;

        let presenter_uid = stream_info
            .extras
            .as_ref()
            .and_then(|extras| extras.get("presenter_uid"))
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        println!(
            "Using WUP to get stream URL: CDN: {}, Stream Name: {}, Presenter UID: {}",
            cdn, stream_name, presenter_uid
        );

        self.get_stream_url_wup(&mut stream_info, &cdn, &stream_name, presenter_uid)
            .await?;

        Ok(stream_info)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use reqwest::Client;
    use rustls::{ClientConfig, crypto::ring};
    use rustls_platform_verifier::BuilderVerifierExt;
    use tokio::stream;

    fn create_client() -> Client {
        let provider = Arc::new(ring::default_provider());
        let tls_config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("Failed to configure default TLS protocol versions")
            .with_platform_verifier()
            .unwrap()
            .with_no_client_auth();

        return Client::builder()
            .use_preconfigured_tls(tls_config)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");
    }

    #[tokio::test]
    #[ignore]
    async fn test_is_live_integration() {
        let extractor =
            HuyaExtractor::new("https://www.huya.com/660000".to_string(), create_client());
        // let mut media_info = extractor.extract().await.unwrap();
        // assert_eq!(media_info.is_live, true);
        // let stream_info = media_info.streams.first().unwrap();
        // assert!(!stream_info.url.is_empty());
        let stream_info = StreamInfo {
            url: "".to_string(),
            format: MediaFormat::Flv,
            quality: "1080p".to_string(),
            bitrate: 0, 
            priority: 0,
            extras: Some(HashMap::from([
                // Add any necessary extras here
                ("cdn".to_string(), "AL".to_string()),
                ("stream_name".to_string(), "78941969-2559461593-10992803837303062528-2693342886-10057-A-0-1-imgplus".to_string()),
                ("presenter_uid".to_string(), "1346609715".to_string()),
            ])),
            codec: "avc".to_string(),
            is_headers_needed: false,
        };

        extractor.get_url(stream_info.clone()).await.unwrap();
        // Manually construct the byte array from the WUP response
        let response_bytes = [
            0, 0, 2, 51, 16, 3, 44, 60, 64, 1, 86, 6, 108, 105, 118, 101, 117, 105, 102, 15, 103, 
            101, 116, 67, 100, 110, 84, 111, 107, 101, 110, 73, 110, 102, 111, 125, 0, 1, 2, 6, 8, 
            0, 2, 6, 0, 29, 0, 0, 1, 12, 6, 4, 116, 82, 115, 112, 29, 0, 1, 1, 241, 10, 6, 0, 22, 
            2, 65, 76, 38, 71, 55, 56, 57, 52, 49, 57, 54, 57, 45, 50, 53, 53, 57, 52, 54, 49, 53, 
            57, 51, 45, 49, 48, 57, 57, 50, 56, 48, 51, 56, 51, 55, 51, 48, 51, 48, 54, 50, 53, 50, 
            56, 45, 50, 54, 57, 51, 51, 52, 50, 56, 56, 54, 45, 49, 48, 48, 53, 55, 45, 65, 45, 48, 
            45, 49, 45, 105, 109, 103, 112, 108, 117, 115, 50, 80, 67, 162, 51, 70, 127, 119, 115, 
            83, 101, 99, 114, 101, 116, 61, 55, 100, 98, 100, 55, 50, 54, 57, 100, 48, 54, 98, 97, 
            102, 56, 53, 53, 56, 98, 102, 55, 50, 55, 56, 98, 50, 52, 97, 53, 101, 100, 97, 38, 119, 
            115, 84, 105, 109, 101, 61, 54, 56, 54, 52, 54, 51, 57, 53, 38, 102, 109, 61, 82, 70, 100, 
            120, 79, 69, 74, 106, 83, 106, 78, 111, 78, 107, 82, 75, 100, 68, 90, 85, 87, 86, 56, 107, 
            77, 70, 56, 107, 77, 86, 56, 107, 77, 108, 56, 107, 77, 119, 37, 51, 68, 38, 99, 116, 121, 
            112, 101, 61, 104, 117, 121, 97, 95, 99, 111, 109, 109, 115, 101, 114, 118, 101, 114, 86, 
            8, 54, 56, 54, 52, 54, 50, 54, 57, 102, 134, 119, 115, 83, 101, 99, 114, 101, 116, 61, 55, 
            100, 98, 100, 55, 50, 54, 57, 100, 48, 54, 98, 97, 102, 56, 53, 53, 56, 98, 102, 55, 50, 
            55, 56, 98, 50, 52, 97, 53, 101, 100, 97, 38, 119, 115, 84, 105, 109, 101, 61, 54, 56, 54, 
            52, 54, 51, 57, 53, 38, 102, 109, 61, 82, 70, 100, 120, 79, 69, 74, 106, 83, 106, 78, 111, 
            78, 107, 82, 75, 100, 68, 90, 85, 87, 86, 56, 107, 77, 70, 56, 107, 77, 86, 56, 107, 77, 
            108, 56, 107, 77, 119, 37, 51, 68, 38, 99, 116, 121, 112, 101, 61, 104, 117, 121, 97, 95, 
            99, 111, 109, 109, 115, 101, 114, 118, 101, 114, 38, 102, 115, 61, 103, 99, 116, 118, 134, 
            119, 115, 83, 101, 99, 114, 101, 116, 61, 55, 100, 98, 100, 55, 50, 54, 57, 100, 48, 54, 
            98, 97, 102, 56, 53, 53, 56, 98, 102, 55, 50, 55, 56, 98, 50, 52, 97, 53, 101, 100, 97, 
            38, 119, 115, 84, 105, 109, 101, 61, 54, 56, 54, 52, 54, 51, 57, 53, 38, 102, 109, 61, 
            82, 70, 100, 120, 79, 69, 74, 106, 83, 106, 78, 111, 78, 107, 82, 75, 100, 68, 90, 85, 
            87, 86, 56, 107, 77, 70, 56, 107, 77, 86, 56, 107, 77, 108, 56, 107, 77, 119, 37, 51, 
            68, 38, 99, 116, 121, 112, 101, 61, 104, 117, 121, 97, 95, 99, 111, 109, 109, 115, 101, 
            114, 118, 101, 114, 38, 102, 115, 61, 103, 99, 116, 11, 140, 152, 12, 168, 12
        ];

        // Test decoding the response
        let mut bytes_mut = bytes::BytesMut::from(&response_bytes[..]);
        let decoded_response = tars_codec::decode_response(&mut bytes_mut).unwrap();

        if let Some(response_message) = decoded_response {
            if let Some(body_bytes) = response_message.body.get("tRsp") {
            let tars_value: TarsValue = tars_codec::de::from_bytes(body_bytes);
            if let Ok(resp) = huya_tars::HuyaGetTokenResp::try_from(tars_value) {
                println!(
                "Test decoded: CDN: {}, Stream Name: {}, FLV Anti Code: {}",
                resp.cdn_type, resp.stream_name, resp.flv_anti_code
                );
            }
            }
        }
    }
}
