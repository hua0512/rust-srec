use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub struct CurrentRoomResponse {
    pub success: bool,
    pub code: Option<i64>,
    pub data: Option<RoomData>,
}

#[derive(Debug, Deserialize)]
pub struct RoomData {
    pub room_info: Option<RoomInfo>,
    pub host_info: Option<HostInfo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct HostInfo {
    pub nick_name: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RoomInfo {
    pub room_id: Option<String>,
    pub room_title: Option<String>,
    pub room_cover: Option<String>,
    pub status: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_pull_config")]
    pub pull_config: Option<PullConfig>,
}

#[derive(Debug, Deserialize)]
pub struct PullConfig {
    pub h265: Option<Vec<serde_json::Value>>,
    pub h264: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

fn deserialize_pull_config<'de, D>(deserializer: D) -> Result<Option<PullConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => serde_json::from_str(&s).map_err(serde::de::Error::custom),
        value => serde_json::from_value(value).map_err(serde::de::Error::custom),
    }
}
