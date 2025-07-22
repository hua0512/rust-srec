#![allow(dead_code, unused_variables)]

use rustc_hash::FxHashMap;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinAppResponse<'a> {
    #[serde(borrow)]
    pub data: DouyinAppResponseData<'a>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinAppResponseData<'a> {
    #[serde(rename = "status_code")]
    pub code: Option<i32>,
    #[serde(borrow)]
    pub prompts: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub message: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub room: Option<DouyinPcData<'a>>,
    #[serde(borrow)]
    pub user: Option<DouyinUserInfo<'a>>,
    #[serde(borrow)]
    pub enter_room_id: Option<&'a str>,
}

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinPcResponse<'a> {
    #[serde(borrow)]
    pub data: DouyinPcResponseData<'a>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinPcData<'a> {
    #[serde(borrow)]
    pub id_str: &'a str,
    pub status: i32,
    #[serde(borrow)]
    pub title: &'a str,
    #[serde(borrow)]
    pub cover: Option<DouyinCover<'a>>,
    #[serde(borrow)]
    pub stream_url: Option<DouyinStreamUrl<'a>>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinPcResponseData<'a> {
    #[serde(borrow)]
    pub prompts: Option<&'a str>,
    #[serde(borrow)]
    pub data: Option<Vec<DouyinPcData<'a>>>,
    #[serde(borrow)]
    pub user: Option<DouyinUserInfo<'a>>,
    #[serde(borrow)]
    pub enter_room_id: Option<&'a str>,

    #[serde(borrow)]
    pub message: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinUserInfo<'a> {
    #[serde(borrow)]
    pub id_str: &'a str,
    #[serde(borrow)]
    pub sec_uid: &'a str,
    #[serde(borrow)]
    pub nickname: &'a str,
    #[serde(borrow)]
    pub avatar_thumb: DouyinAvatarThumb<'a>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinAvatarThumb<'a> {
    #[serde(borrow)]
    pub url_list: Vec<Cow<'a, str>>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinCover<'a> {
    #[serde(borrow)]
    pub url_list: Vec<Cow<'a, str>>,
}

#[derive(Deserialize, Debug)]
pub struct DouyinStreamUrl<'a> {
    #[serde(borrow)]
    pub flv_pull_url: FxHashMap<Cow<'a, str>, Cow<'a, str>>,
    #[serde(borrow)]
    pub default_resolution: &'a str,
    #[serde(borrow)]
    pub hls_pull_url_map: FxHashMap<Cow<'a, str>, Cow<'a, str>>,
    #[serde(borrow)]
    pub hls_pull_url: Cow<'a, str>,
    pub stream_orientation: i32,
    #[serde(borrow)]
    pub live_core_sdk_data: DouyinLiveCoreSdkData<'a>,
    pub extra: DouyinStreamExtra,
    #[serde(deserialize_with = "deserialize_pull_datas")]
    pub pull_datas: DouyinPullDatas,
}

#[derive(Debug)]
pub(crate) struct DouyinPullDatas {
    pub data: FxHashMap<String, DouyinPullDataEntry>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinPullDataEntry {
    pub url: String,
    #[serde(rename = "type", default)]
    pub format: String,
    pub quality: Option<i32>,
    pub bitrate: Option<i64>,
    pub codec: Option<String>,
    #[serde(flatten)]
    pub extra_params: FxHashMap<String, serde_json::Value>,
}

// Custom deserializer for pull_datas
fn deserialize_pull_datas<'de, D>(deserializer: D) -> Result<DouyinPullDatas, D::Error>
where
    D: Deserializer<'de>,
{
    // First deserialize as a generic map
    let map: FxHashMap<String, serde_json::Value> = FxHashMap::deserialize(deserializer)?;
    let mut data = FxHashMap::default();

    // Try to convert each value to DouyinPullDataEntry
    for (key, value) in map {
        if let Ok(entry) = serde_json::from_value::<DouyinPullDataEntry>(value) {
            data.insert(key, entry);
        }
        // If conversion fails, we just skip this entry
    }

    Ok(DouyinPullDatas { data })
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinLiveCoreSdkData<'a> {
    #[serde(borrow)]
    pub pull_data: DouyinSdkPullData<'a>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinSdkPullData<'a> {
    #[serde(borrow)]
    pub options: DouyinStreamOptions<'a>,
    #[serde(deserialize_with = "deserialize_stream_data")]
    pub stream_data: DouyinStreamDataParsed,
}

#[derive(Debug)]
pub(crate) struct DouyinStreamDataParsed {
    pub common: Option<DouyinStreamDataCommon>,
    pub data: FxHashMap<String, DouyinStreamDataQuality>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinStreamDataCommon {
    pub ts: String,
    pub session_id: String,
    pub stream: String,
    pub version: i32,
    pub rule_ids: String,
    pub common_trace: String,
    pub app_id: String,
    pub major_anchor_level: String,
    pub mode: String,
    pub lines: serde_json::Value,
    pub p2p_params: serde_json::Value,
    pub stream_data_content_encoding: String,
    pub common_sdk_params: serde_json::Value,
    pub stream_name: String,
    pub main_push_id: i32,
    pub backup_push_id: i32,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinStreamDataQuality {
    pub main: DouyinStreamDataMain,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinStreamDataMain {
    pub flv: String,
    pub hls: String,
    pub cmaf: String,
    pub dash: String,
    pub lls: String,
    pub tsl: String,
    pub tile: String,
    pub http_ts: String,
    pub ll_hls: String,
    pub sdk_params: String,
    #[serde(rename = "enableEncryption")]
    pub enable_encryption: bool,
}

// Custom deserializer for stream_data
fn deserialize_stream_data<'de, D>(deserializer: D) -> Result<DouyinStreamDataParsed, D::Error>
where
    D: Deserializer<'de>,
{
    // Deserialize as a string first
    let json_str: String = String::deserialize(deserializer)?;

    // If the string is empty or just "{}", return empty data
    if json_str.trim().is_empty() || json_str.trim() == "{}" {
        return Ok(DouyinStreamDataParsed {
            common: None,
            data: FxHashMap::default(),
        });
    }

    // Try to parse the JSON string
    match serde_json::from_str::<serde_json::Value>(&json_str) {
        Ok(value) => {
            let mut result = DouyinStreamDataParsed {
                common: None,
                data: FxHashMap::default(),
            };

            if let serde_json::Value::Object(obj) = value {
                // Parse common section if it exists
                if let Some(common_value) = obj.get("common") {
                    if let Ok(common) =
                        serde_json::from_value::<DouyinStreamDataCommon>(common_value.clone())
                    {
                        result.common = Some(common);
                    }
                }

                // Parse data section if it exists
                if let Some(serde_json::Value::Object(data_obj)) = obj.get("data") {
                    for (quality_key, quality_value) in data_obj {
                        if let Ok(quality_data) =
                            serde_json::from_value::<DouyinStreamDataQuality>(quality_value.clone())
                        {
                            result.data.insert(quality_key.clone(), quality_data);
                        }
                    }
                }
            }

            Ok(result)
        }
        Err(_) => {
            // If parsing fails, return empty data structure
            Ok(DouyinStreamDataParsed {
                common: None,
                data: FxHashMap::default(),
            })
        }
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinStreamOptions<'a> {
    #[serde(borrow)]
    pub default_quality: DouyinQuality<'a>,
    #[serde(borrow)]
    pub qualities: Vec<DouyinQuality<'a>>,
}

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinQuality<'a> {
    #[serde(borrow)]
    pub name: &'a str,
    #[serde(borrow)]
    pub sdk_key: &'a str,
    #[serde(borrow)]
    pub v_codec: &'a str,
    #[serde(borrow)]
    pub resolution: &'a str,
    pub level: i32,
    pub v_bit_rate: i32,
    #[serde(borrow)]
    pub additional_content: &'a str,
    pub fps: i32,
    pub disable: i32,
}

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinStreamExtra {
    pub height: i32,
    pub width: i32,
    pub fps: i32,
    pub max_bitrate: i64,
    pub min_bitrate: i64,
    pub default_bitrate: i64,
    pub bitrate_adapt_strategy: i32,
    pub anchor_interact_profile: i32,
    pub audience_interact_profile: i32,
    pub hardware_encode: bool,
    pub video_profile: i32,
    pub h265_enable: bool,
    pub gop_sec: i32,
    pub bframe_enable: bool,
    pub roi: bool,
    pub sw_roi: bool,
    pub bytevc1_enable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DouyinStreamExtras {
    pub resolution: String,
    pub sdk_key: String,
}

// Additional structs for parsing the stream_data JSON if needed
#[derive(Deserialize, Debug)]

pub(crate) struct DouyinStreamData<'a> {
    #[serde(borrow)]
    pub common: DouyinStreamCommon<'a>,
    #[serde(borrow)]
    pub data: FxHashMap<&'a str, DouyinStreamQualityData<'a>>,
}

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinStreamCommon<'a> {
    #[serde(borrow)]
    pub ts: &'a str,
    #[serde(borrow)]
    pub session_id: &'a str,
    #[serde(borrow)]
    pub stream: &'a str,
    pub version: i32,
    #[serde(borrow)]
    pub rule_ids: &'a str,
    #[serde(borrow)]
    pub common_trace: &'a str,
    #[serde(borrow)]
    pub app_id: &'a str,
    #[serde(borrow)]
    pub major_anchor_level: &'a str,
    #[serde(borrow)]
    pub mode: &'a str,
    pub lines: serde_json::Value,
    pub p2p_params: serde_json::Value,
    #[serde(borrow)]
    pub stream_data_content_encoding: &'a str,
    pub common_sdk_params: serde_json::Value,
    #[serde(borrow)]
    pub stream_name: &'a str,
    pub main_push_id: i32,
    pub backup_push_id: i32,
}

#[derive(Deserialize, Debug)]

pub(crate) struct DouyinStreamQualityData<'a> {
    #[serde(borrow)]
    pub main: DouyinStreamMain<'a>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct DouyinStreamMain<'a> {
    #[serde(borrow)]
    pub flv: &'a str,
    #[serde(borrow)]
    pub hls: &'a str,
    #[serde(borrow)]
    pub cmaf: &'a str,
    #[serde(borrow)]
    pub dash: &'a str,
    #[serde(borrow)]
    pub lls: &'a str,
    #[serde(borrow)]
    pub tsl: &'a str,
    #[serde(borrow)]
    pub tile: &'a str,
    #[serde(borrow)]
    pub http_ts: &'a str,
    #[serde(borrow)]
    pub ll_hls: &'a str,
    #[serde(borrow)]
    pub sdk_params: &'a str,
    #[serde(rename = "enableEncryption")]
    pub enable_encryption: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pull_datas_deserialize_empty() {
        let json_str = r#"{
            "flv_pull_url": {
                "HD1": "http://example.com/hd.flv"
            },
            "default_resolution": "HD1",
            "hls_pull_url_map": {
                "HD1": "http://example.com/hd.m3u8"
            },
            "hls_pull_url": "http://example.com/hd.m3u8",
            "stream_orientation": 1,
            "live_core_sdk_data": {
                "pull_data": {
                    "options": {
                        "default_quality": {
                            "name": "HD",
                            "sdk_key": "hd",
                            "v_codec": "h264",
                            "resolution": "1280x720",
                            "level": 2,
                            "v_bit_rate": 2000000,
                            "additional_content": "",
                            "fps": 30,
                            "disable": 0
                        },
                        "qualities": []
                    },
                    "stream_data": "{}"
                }
            },
            "extra": {
                "height": 720,
                "width": 1280,
                "fps": 30,
                "max_bitrate": 0,
                "min_bitrate": 0,
                "default_bitrate": 0,
                "bitrate_adapt_strategy": 0,
                "anchor_interact_profile": 0,
                "audience_interact_profile": 0,
                "hardware_encode": false,
                "video_profile": 0,
                "h265_enable": false,
                "gop_sec": 2,
                "bframe_enable": false,
                "roi": false,
                "sw_roi": false,
                "bytevc1_enable": false
            },
            "pull_datas": {}
        }"#;

        let result: Result<DouyinStreamUrl, _> = serde_json::from_str(json_str);
        assert!(result.is_ok());
        let stream_url = result.unwrap();
        assert!(stream_url.pull_datas.data.is_empty());
    }

    #[test]
    fn test_pull_datas_deserialize_with_data() {
        let json_str = r#"{
            "flv_pull_url": {
                "HD1": "http://example.com/hd.flv"
            },
            "default_resolution": "HD1",
            "hls_pull_url_map": {
                "HD1": "http://example.com/hd.m3u8"
            },
            "hls_pull_url": "http://example.com/hd.m3u8",
            "stream_orientation": 1,
            "live_core_sdk_data": {
                "pull_data": {
                    "options": {
                        "default_quality": {
                            "name": "HD",
                            "sdk_key": "hd",
                            "v_codec": "h264",
                            "resolution": "1280x720",
                            "level": 2,
                            "v_bit_rate": 2000000,
                            "additional_content": "",
                            "fps": 30,
                            "disable": 0
                        },
                        "qualities": []
                    },
                    "stream_data": "{}"
                }
            },
            "extra": {
                "height": 720,
                "width": 1280,
                "fps": 30,
                "max_bitrate": 0,
                "min_bitrate": 0,
                "default_bitrate": 0,
                "bitrate_adapt_strategy": 0,
                "anchor_interact_profile": 0,
                "audience_interact_profile": 0,
                "hardware_encode": false,
                "video_profile": 0,
                "h265_enable": false,
                "gop_sec": 2,
                "bframe_enable": false,
                "roi": false,
                "sw_roi": false,
                "bytevc1_enable": false
            },
            "pull_datas": {
                "stream1": {
                    "url": "http://example.com/stream1.flv",
                    "type": "flv",
                    "quality": 720,
                    "bitrate": 2000000,
                    "codec": "h264"
                },
                "stream2": {
                    "url": "http://example.com/stream2.m3u8",
                    "type": "hls",
                    "quality": 1080,
                    "bitrate": 4000000,
                    "codec": "h264"
                }
            }
        }"#;

        let result: Result<DouyinStreamUrl, _> = serde_json::from_str(json_str);
        assert!(result.is_ok());
        let stream_url = result.unwrap();
        assert_eq!(stream_url.pull_datas.data.len(), 2);
        assert!(stream_url.pull_datas.data.contains_key("stream1"));
        assert!(stream_url.pull_datas.data.contains_key("stream2"));

        let stream1 = &stream_url.pull_datas.data["stream1"];
        assert_eq!(stream1.url, "http://example.com/stream1.flv");
        assert_eq!(stream1.format, "flv");
        assert_eq!(stream1.quality, Some(720));
    }

    #[test]
    fn test_stream_data_deserialize_empty() {
        let empty_data = r#"{
            "options": {
                "default_quality": {
                    "name": "HD",
                    "sdk_key": "hd",
                    "v_codec": "h264",
                    "resolution": "1280x720",
                    "level": 2,
                    "v_bit_rate": 2000000,
                    "additional_content": "",
                    "fps": 30,
                    "disable": 0
                },
                "qualities": []
            },
            "stream_data": "{}"
        }"#;

        let result: Result<DouyinSdkPullData, _> = serde_json::from_str(empty_data);
        assert!(result.is_ok(), "Should deserialize empty stream_data");

        let pull_data = result.unwrap();
        assert!(pull_data.stream_data.common.is_none());
        assert!(pull_data.stream_data.data.is_empty());
    }

    #[test]
    fn test_stream_data_deserialize_with_data() {
        let data_with_content = r#"{
            "options": {
                "default_quality": {
                    "name": "HD",
                    "sdk_key": "hd", 
                    "v_codec": "h264",
                    "resolution": "1280x720",
                    "level": 2,
                    "v_bit_rate": 2000000,
                    "additional_content": "",
                    "fps": 30,
                    "disable": 0
                },
                "qualities": []
            },
            "stream_data": "{\"common\":{\"ts\":\"1751545222\",\"session_id\":\"test-session\",\"stream\":\"405774431831196265\",\"version\":0,\"rule_ids\":\"{}\",\"common_trace\":\"{}\",\"app_id\":\"100100\",\"major_anchor_level\":\"common\",\"mode\":\"Normal\",\"lines\":{},\"p2p_params\":{},\"stream_data_content_encoding\":\"default\",\"common_sdk_params\":{},\"stream_name\":\"stream-405774431831196265\",\"main_push_id\":617,\"backup_push_id\":0},\"data\":{\"hd\":{\"main\":{\"flv\":\"http://example.com/hd.flv\",\"hls\":\"http://example.com/hd.m3u8\",\"cmaf\":\"\",\"dash\":\"\",\"lls\":\"\",\"tsl\":\"\",\"tile\":\"\",\"http_ts\":\"\",\"ll_hls\":\"\",\"sdk_params\":\"{}\",\"enableEncryption\":false}}}}"
        }"#;

        let result: Result<DouyinSdkPullData, _> = serde_json::from_str(data_with_content);
        assert!(
            result.is_ok(),
            "Should deserialize stream_data with content"
        );

        let pull_data = result.unwrap();
        assert!(pull_data.stream_data.common.is_some());
        assert!(!pull_data.stream_data.data.is_empty());

        let common = pull_data.stream_data.common.as_ref().unwrap();
        assert_eq!(common.ts, "1751545222");
        assert_eq!(common.session_id, "test-session");

        assert!(pull_data.stream_data.data.contains_key("hd"));
        let hd_quality = &pull_data.stream_data.data["hd"];
        assert_eq!(hd_quality.main.flv, "http://example.com/hd.flv");
        assert_eq!(hd_quality.main.hls, "http://example.com/hd.m3u8");
        assert!(!hd_quality.main.enable_encryption);
    }
}
