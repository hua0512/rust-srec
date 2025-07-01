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

        let streams = self.parse_streams(stream_info_list, bitrate_info_list, default_bitrate)?;

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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use reqwest::Client;
    use rustls::{ClientConfig, crypto::ring};
    use rustls_platform_verifier::BuilderVerifierExt;

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
        let media_info = extractor.extract().await.unwrap();
        assert_eq!(media_info.is_live, true);
    }
}
