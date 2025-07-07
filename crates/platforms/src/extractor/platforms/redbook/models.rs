#![allow(unused)]
use std::borrow::Cow;

use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveInfo<'a> {
    #[serde(borrow)]
    pub live_stream: LiveStream<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStream<'a> {
    #[serde(borrow)]
    pub page_status: Cow<'a, str>,
    #[serde(borrow)]
    pub live_status: Cow<'a, str>,
    #[serde(borrow)]
    pub error_message: Cow<'a, str>,

    #[serde(borrow)]
    pub room_data: RoomData<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomData<'a> {
    #[serde(borrow)]
    pub host_info: HostInfo<'a>,
    #[serde(borrow)]
    pub room_info: RoomInfo<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo<'a> {
    #[serde(borrow)]
    pub avatar: Cow<'a, str>,
    #[serde(borrow)]
    pub nick_name: Cow<'a, str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo<'a> {
    #[serde(deserialize_with = "deserialize_pull_config")]
    pub pull_config: Option<PullConfig>,
    #[serde(borrow)]
    pub deeplink: Cow<'a, str>,

    #[serde(borrow)]
    pub room_cover: Cow<'a, str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullConfig {
    pub h265: Vec<serde_json::Value>,
    pub h264: Vec<serde_json::Value>,
    pub width: u32,
    pub height: u32,
}

fn deserialize_pull_config<'de, D>(deserializer: D) -> Result<Option<PullConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;

    match value {
        serde_json::Value::Object(_) => {
            let config: PullConfig =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Some(config))
        }
        serde_json::Value::String(s) => {
            // Parse the JSON string first, then deserialize as PullConfig
            let parsed_value: serde_json::Value =
                serde_json::from_str(&s).map_err(serde::de::Error::custom)?;
            let config: PullConfig =
                serde_json::from_value(parsed_value).map_err(serde::de::Error::custom)?;
            Ok(Some(config))
        }
        _ => Ok(None),
    }
}
