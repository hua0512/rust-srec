#![allow(dead_code)]

//! Serde models for Bigo studio + WebSocket link APIs.

use serde::Deserialize;

/// Top-level studio API response.
#[derive(Debug, Deserialize)]
pub struct StudioResponse {
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub data: Option<StudioData>,
}

impl StudioResponse {
    pub fn is_success(&self) -> bool {
        match &self.code {
            None => true,
            Some(serde_json::Value::Number(n)) => n.as_i64() == Some(0),
            Some(serde_json::Value::String(s)) => s == "0",
            Some(serde_json::Value::Null) => true,
            _ => false,
        }
    }
}

/// Studio `data` object. Large ids stay as strings.
#[derive(Debug, Deserialize, Default)]
pub struct StudioData {
    #[serde(default, deserialize_with = "de_opt_i64ish")]
    pub alive: Option<i64>,
    #[serde(rename = "roomId", default, deserialize_with = "de_opt_stringish")]
    pub room_id: Option<String>,
    #[serde(rename = "siteId", default, deserialize_with = "de_opt_stringish")]
    pub site_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_stringish")]
    pub sid: Option<String>,
    #[serde(default, deserialize_with = "de_opt_stringish")]
    pub uid: Option<String>,
    #[serde(default)]
    pub nick_name: Option<String>,
    #[serde(rename = "roomTopic", default)]
    pub room_topic: Option<String>,
    #[serde(rename = "gameTitle", default)]
    pub game_title: Option<String>,
    #[serde(default)]
    pub hls_src: Option<String>,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(rename = "passRoom", default)]
    pub pass_room: Option<bool>,
    #[serde(
        rename = "clientBigoId",
        default,
        deserialize_with = "de_opt_stringish"
    )]
    pub client_bigo_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i64ish")]
    pub reserver: Option<i64>,
    #[serde(rename = "country_code", default)]
    pub country_code: Option<String>,
}

impl StudioData {
    pub fn is_online(&self) -> bool {
        let alive = self.alive.unwrap_or(0) != 0;
        let hls = self
            .hls_src
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let room_id = self.room_id.as_deref().unwrap_or("");
        alive && hls && !room_id.is_empty() && room_id != "0"
    }

    pub fn display_title(&self) -> String {
        self.room_topic
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.game_title.as_deref().filter(|s| !s.is_empty()))
            .or_else(|| self.nick_name.as_deref().filter(|s| !s.is_empty()))
            .or(self.site_id.as_deref())
            .unwrap_or("Bigo Live")
            .to_string()
    }

    pub fn artist(&self) -> String {
        self.nick_name
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.client_bigo_id.clone())
            .or_else(|| self.site_id.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// Response from `getWebSocketLink`.
#[derive(Debug, Deserialize)]
pub struct WsLinkResponse {
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    #[serde(default)]
    pub data: Option<WsLinkData>,
}

impl WsLinkResponse {
    pub fn is_success(&self) -> bool {
        match &self.code {
            None => true,
            Some(serde_json::Value::Number(n)) => n.as_i64() == Some(0),
            Some(serde_json::Value::String(s)) => s == "0",
            Some(serde_json::Value::Null) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct WsLinkData {
    #[serde(rename = "uidToken", default)]
    pub uid_token: Option<String>,
    #[serde(rename = "userId", default, deserialize_with = "de_opt_stringish")]
    pub user_id: Option<String>,
    #[serde(rename = "userName", default, deserialize_with = "de_opt_stringish")]
    pub user_name: Option<String>,
    #[serde(rename = "deviceId", default)]
    pub device_id: Option<String>,
    #[serde(rename = "seqId", default, deserialize_with = "de_opt_stringish")]
    pub seq_id: Option<String>,
}

fn de_opt_stringish<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

fn de_opt_i64ish<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        Some(serde_json::Value::Bool(b)) => Some(if b { 1 } else { 0 }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_fixture() {
        let json = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "sid": 2318878816,
            "siteId": "221338632",
            "uid": 1739055289,
            "nick_name": "Streamer",
            "gameTitle": "",
            "roomTopic": "Hello",
            "snapshot": "https://example.com/snap.jpg",
            "alive": 1,
            "roomId": "6576287577575737440",
            "hls_src": "https://cdn.example.com/list_x_0.m3u8",
            "passRoom": false,
            "avatar": "https://example.com/a.jpg",
            "clientBigoId": "221338632",
            "reserver": 4108
          }
        }"#;
        let resp: StudioResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        let data = resp.data.unwrap();
        assert!(data.is_online());
        assert_eq!(data.room_id.as_deref(), Some("6576287577575737440"));
        assert_eq!(data.display_title(), "Hello");
        assert_eq!(data.artist(), "Streamer");
    }

    #[test]
    fn offline_fixture() {
        let json = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "alive": 0,
            "roomId": "0",
            "hls_src": "",
            "nick_name": "OfflineBJ",
            "siteId": "123"
          }
        }"#;
        let resp: StudioResponse = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert!(!data.is_online());
        assert_eq!(data.artist(), "OfflineBJ");
    }

    #[test]
    fn string_code_and_room_id() {
        let json = r#"{"code":"0","data":{"alive":"1","roomId":7482488749430738201,"hls_src":"https://x/list.m3u8","nick_name":"n"}}"#;
        let resp: StudioResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        let data = resp.data.unwrap();
        assert!(data.is_online());
        assert_eq!(data.room_id.as_deref(), Some("7482488749430738201"));
    }
}
