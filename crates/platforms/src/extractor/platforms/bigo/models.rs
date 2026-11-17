//! Serde models for the Bigo profile, studio and WebSocket link APIs.

use serde::Deserialize;
use url::Url;

/// Response from the public profile API, `getUserInfoStudio`.
#[derive(Debug, Deserialize)]
pub struct PresenceResponse {
    /// 0 on success, 1 for an unknown id.
    pub code: i64,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub data: Option<Presence>,
}

/// A broadcaster's profile and live state.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    /// 1 while the broadcaster is live.
    pub alive: i64,
    /// The broadcaster's numeric account id.
    pub uid: u64,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub room_topic: Option<String>,
    /// The broadcaster's profile picture, also while offline.
    #[serde(default)]
    pub og_image: Option<String>,
    /// The room's broadcast state, read through [`Presence::is_app_only`].
    #[serde(default, deserialize_with = "de_opt_i64ish")]
    room_status: Option<i64>,
    /// App link to the room,
    /// `bigolive://livevideoshow?roomid=<room id>&uid=<uid>&visitor=1`, with
    /// `roomid=0` while offline.
    #[serde(rename = "aliOSUrl", default)]
    pub ali_os_url: Option<String>,
}

/// The profile's `roomStatus` for a room broadcast for the app only: the
/// website reads it as offline or without a stream.
const APP_ONLY_ROOM: i64 = 4;

impl Presence {
    /// Whether the room is broadcast for the app only, without a web stream.
    pub fn is_app_only(&self) -> bool {
        self.room_status == Some(APP_ONLY_ROOM)
    }

    /// The id of the broadcaster's live room, read from the app link.
    ///
    /// Only a `bigolive://livevideoshow` link whose `uid` is this
    /// broadcaster's and whose `roomid` is not 0 names a room; anything else
    /// gives `None`.
    pub fn room_id(&self) -> Option<u64> {
        let url = Url::parse(self.ali_os_url.as_deref()?).ok()?;
        if url.scheme() != "bigolive" || url.host_str() != Some("livevideoshow") {
            return None;
        }
        let (mut room_id, mut uid) = (None, None);
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "roomid" => room_id = value.parse::<u64>().ok(),
                "uid" => uid = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        room_id.filter(|&room_id| room_id != 0 && uid == Some(self.uid))
    }
}

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
    #[serde(default)]
    pub hls_src: Option<String>,
    #[serde(rename = "passRoom", default)]
    pub pass_room: Option<bool>,
    /// Set when Bigo wants a signed-in session before it answers; `alive`
    /// and `roomId` then read as offline whatever the room is doing.
    #[serde(rename = "needLogin", default)]
    pub need_login: Option<bool>,
}

impl StudioData {
    /// Whether the studio reports the room as broadcasting, from `alive` and
    /// `roomId`.
    pub fn is_room_live(&self) -> bool {
        let alive = self.alive.unwrap_or(0) != 0;
        let room_id = self.room_id.as_deref().unwrap_or("");
        alive && !room_id.is_empty() && room_id != "0"
    }

    /// Whether the response carried a playlist URL to hand to the downloader.
    pub fn has_playable_source(&self) -> bool {
        self.hls_src.as_deref().is_some_and(|s| !s.is_empty())
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
    /// The signed-in account's identity, in the same fields; absent unless
    /// the request carried a signed-in session.
    #[serde(rename = "realUser", default)]
    pub real_user: Option<Box<WsLinkData>>,
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
    fn string_code_and_room_id() {
        let json = r#"{"code":"0","data":{"alive":"1","roomId":7482488749430738201,"hls_src":"https://x/list.m3u8","nick_name":"n"}}"#;
        let resp: StudioResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        let data = resp.data.unwrap();
        assert!(data.is_room_live());
        assert!(data.has_playable_source());
        assert_eq!(data.room_id.as_deref(), Some("7482488749430738201"));
    }

    #[test]
    fn room_id_comes_only_from_the_broadcasters_own_room_link() {
        let presence = |link: &str| Presence {
            alive: 1,
            uid: 300000003,
            nickname: None,
            room_topic: None,
            og_image: None,
            room_status: None,
            ali_os_url: Some(link.to_owned()),
        };
        assert_eq!(
            presence("bigolive://livevideoshow?roomid=7000000000000000002&uid=300000003&visitor=1")
                .room_id(),
            Some(7000000000000000002)
        );
        for link in [
            "bigolive://livevideoshow?roomid=0&uid=300000003&visitor=1",
            "bigolive://livevideoshow?roomid=7000000000000000002&uid=1&visitor=1",
            "bigolive://livevideoshow?roomid=7000000000000000002",
            "bigolive://otherpage?roomid=7000000000000000002&uid=300000003",
            "https://livevideoshow/?roomid=7000000000000000002&uid=300000003",
            "bigolive://livevideoshow?roomid=abc&uid=300000003",
        ] {
            assert_eq!(presence(link).room_id(), None, "{link}");
        }
    }
}
