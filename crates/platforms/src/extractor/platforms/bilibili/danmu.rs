//! Bilibili (哔哩哔哩) danmu provider.
//!
//! Implements danmu collection for Bilibili live streaming using the generic
//! WebSocket provider with binary protocol and Brotli/Zlib compression.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;
use chrono::{TimeZone, Utc};
use flate2::read::ZlibDecoder;
use prost::Message as ProstMessage;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::tungstenite::http::HeaderMap;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::ws_headers_origin_referer_ua;
use crate::danmaku::websocket::{
    DanmuProtocol, DanmuProtocolFactory, DanmuProtocolOutput, WebSocketDanmuProvider,
};
use crate::danmaku::{DanmuControlEvent, DanmuItem, DanmuMessage};
use crate::extractor::default::{DEFAULT_UA, default_client};

use super::URL_REGEX;
use super::cookie_utils::{extract_cookie_value, strip_refresh_token};
use super::utils::generate_fake_buvid3;
use super::wbi::{encode_wbi, get_wbi_keys};
use crate::extractor::utils::capture_group_1_owned;

/// Default WebSocket URL
const DEFAULT_WS_URL: &str = "wss://broadcastlv.chat.bilibili.com/sub";

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Heartbeat packet (operation = 2)
/// Header: len=31, header_len=16, ver=1, op=2, seq=1
/// Body: "[object Object]"
const HEARTBEAT: &[u8] = &[
    0x00, 0x00, 0x00, 0x1f, // packet length = 31
    0x00, 0x10, // header length = 16
    0x00, 0x01, // version = 1
    0x00, 0x00, 0x00, 0x02, // operation = 2 (heartbeat)
    0x00, 0x00, 0x00, 0x01, // sequence = 1
    // "[object Object]"
    0x5b, 0x6f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x4f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x5d,
];

/// Operation codes
#[expect(
    dead_code,
    reason = "retained for protocol variants and forward-compatible response handling"
)]
mod op {
    pub const HEARTBEAT_REPLY: u32 = 3;
    pub const NOTIFICATION: u32 = 5;
    pub const AUTH: u32 = 7;
    pub const AUTH_REPLY: u32 = 8;
}

/// Protocol versions
mod ver {
    pub const RAW_JSON: u16 = 0;
    pub const POPULARITY: u16 = 1;
    pub const ZLIB: u16 = 2;
    pub const BROTLI: u16 = 3;
}

/// Room init API response
#[derive(Debug, Deserialize)]
struct RoomInitResponse {
    code: i32,
    data: Option<RoomInitData>,
}

#[derive(Debug, Deserialize)]
struct RoomInitData {
    room_id: u64,
}

/// Authentication data sent to WebSocket
#[derive(Debug, Serialize)]
struct AuthData {
    uid: u64,
    roomid: u64,
    protover: u8,
    platform: &'static str,
    #[serde(rename = "type")]
    auth_type: u8,
    key: String,
}

/// Decoded packet
struct DecodedPacket {
    operation: u32,
    /// Slice of the frame (or of the decompressed buffer) the packet came
    /// from; refcounted, so packets never copy their body.
    body: Bytes,
}

// Borrow the Base64 payload and skip unrelated JSON fields on the common V2
// path. Cow still accepts escaped strings, allocating only when unescaping.
#[derive(Deserialize)]
struct GiftV2Notification<'a> {
    #[serde(borrow)]
    cmd: Cow<'a, str>,
    #[serde(borrow)]
    data: GiftV2Payload<'a>,
}

#[derive(Deserialize)]
struct GiftV2Payload<'a> {
    #[serde(borrow)]
    pb: Cow<'a, str>,
}

// Subset of SEND_GIFT_V2 used for recording. Tags follow the schema shipped in
// blive-message-listener@0.5.6-beta.1 (src/protobuf/SEND_GIFT_V2.proto.ts).
// Prost skips other fields, including nested medal and animation information.
#[derive(Clone, PartialEq, prost::Message)]
struct SendGiftV2 {
    #[prost(int64, tag = "1")]
    uid: i64,
    #[prost(string, tag = "2")]
    uname: String,
    #[prost(message, repeated, tag = "10")]
    gift_list: Vec<GiftV2>,
    #[prost(bool, tag = "11")]
    switch: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
struct GiftV2 {
    #[prost(string, tag = "2")]
    gift_name: String,
    #[prost(int64, tag = "3")]
    num: i64,
    #[prost(int64, tag = "5")]
    price: i64,
    #[prost(string, tag = "8")]
    coin_type: String,
    #[prost(int64, tag = "10")]
    timestamp: i64,
}

/// Bilibili Danmu Protocol Implementation
#[derive(Clone)]
pub struct BilibiliDanmuProtocol {
    client: Client,
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
    uid: Option<u64>,
    connection_cookies: Option<String>,
}

impl Default for BilibiliDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            cookies: None,
            uid: None,
            connection_cookies: None,
        }
    }
}

impl BilibiliDanmuProtocol {
    /// Create a new BilibiliDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new BilibiliDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            client: default_client(),
            cookies: Some(cookies.into()),
            uid: None,
            connection_cookies: None,
        }
    }

    fn build_cookie_header(user_cookies: Option<&str>, fallback_buvid3: &str) -> String {
        let mut buvid3_value: Option<String> = None;
        let mut other_parts: Vec<String> = Vec::new();

        if let Some(user_cookies) = user_cookies {
            for part in user_cookies.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }

                let mut kv = part.splitn(2, '=');
                let name = kv.next().unwrap_or("").trim();
                let value = kv.next().map(str::trim);

                if name.eq_ignore_ascii_case("buvid3") {
                    if buvid3_value.is_none() && matches!(value, Some(v) if !v.is_empty()) {
                        buvid3_value = value.map(ToString::to_string);
                    }
                    continue;
                }

                other_parts.push(part.to_string());
            }
        }

        let buvid3_value = buvid3_value.unwrap_or_else(|| fallback_buvid3.to_string());

        if other_parts.is_empty() {
            format!("buvid3={}", buvid3_value)
        } else {
            format!("buvid3={}; {}", buvid3_value, other_parts.join("; "))
        }
    }

    fn http_cookie_header(&self) -> Option<String> {
        if let Some(cookies) = self.connection_cookies.as_deref() {
            return Some(strip_refresh_token(cookies));
        }
        self.cookies
            .as_deref()
            .map(|c| strip_refresh_token(&self.normalize_cookies(c)))
    }

    /// Get real room ID from short ID.
    async fn get_real_room_id(&self, short_id: &str) -> Result<u64> {
        let url = format!(
            "https://api.live.bilibili.com/room/v1/Room/room_init?id={}",
            short_id
        );

        let resp: RoomInitResponse = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://live.bilibili.com")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get room info: {}", e)))?
            .json()
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to parse room info: {}", e)))?;

        if resp.code != 0 {
            return Err(DanmakuError::protocol(
                "Room init API returned error".to_string(),
            ));
        }

        resp.data
            .map(|d| d.room_id)
            .ok_or_else(|| DanmakuError::protocol("No room data in response".to_string()))
    }

    /// Get danmaku connection info (WebSocket URL and token).
    async fn get_danmu_info(&self, room_id: u64) -> Result<(String, String)> {
        // Build params
        let params = vec![
            ("id", room_id.to_string()),
            ("type", "0".to_string()),
            ("web_location", "444.8".to_string()),
        ];

        // Sign with WBI
        let keys = get_wbi_keys(&self.client)
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to get WBI keys: {}", e)))?;

        let query_string = encode_wbi(params, keys)
            .map_err(|e| DanmakuError::protocol(format!("Failed to encode WBI: {}", e)))?;

        let url = format!(
            "https://api.live.bilibili.com/xlive/web-room/v1/index/getDanmuInfo?{}",
            query_string
        );

        debug!("getDanmuInfo URL: {}", url);

        // Make request
        let mut req = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://live.bilibili.com");
        if let Some(cookie_header) = self.http_cookie_header() {
            req = req.header(reqwest::header::COOKIE, cookie_header);
        }

        let response = req
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get danmu info: {}", e)))?
            .text()
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to read response: {}", e)))?;

        // Parse JSON response
        let json: Value = serde_json::from_str(&response)
            .map_err(|e| DanmakuError::protocol(format!("Invalid JSON: {}", e)))?;

        // Check if API returned an error
        let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            debug!(
                "getDanmuInfo returned error code {}: {}, using default WebSocket URL",
                code, msg
            );
            return Ok((DEFAULT_WS_URL.to_string(), String::new()));
        }

        // Parse successful response
        let data = json
            .get("data")
            .ok_or_else(|| DanmakuError::protocol("Missing data field in response".to_string()))?;

        let token = data
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let ws_url = data
            .get("host_list")
            .and_then(|v| v.as_array())
            .and_then(|list| list.first())
            .and_then(|host| {
                let h = host.get("host")?.as_str()?;
                let p = host.get("wss_port")?.as_u64()?;
                Some(format!("wss://{}:{}/sub", h, p))
            })
            .unwrap_or_else(|| DEFAULT_WS_URL.to_string());

        debug!("Got token and WebSocket URL: {}", ws_url);
        Ok((ws_url, token))
    }

    /// Build authentication packet.
    fn build_auth_packet(&self, room_id: u64, token: &str) -> Bytes {
        // If `uid` is set to a non-zero user mid, the token must be obtained with
        // matching authenticated cookies (otherwise the server may reset the connection).
        let uid = if self.uid.is_some()
            && self
                .connection_cookies
                .as_deref()
                .and_then(|c| extract_cookie_value(c, "SESSDATA"))
                .is_some()
        {
            self.uid.unwrap_or(0)
        } else {
            0
        };
        let auth_data = AuthData {
            uid,
            roomid: room_id,
            protover: 3, // Request Brotli compression
            platform: "web",
            auth_type: 2,
            key: token.to_string(),
        };

        // `AuthData` is fully serializable; in the unlikely event of serialization failure,
        // send an empty payload rather than panic.
        let json_data = serde_json::to_vec(&auth_data).unwrap_or_default();
        Bytes::from(build_packet(&json_data, op::AUTH))
    }

    /// Decode packets, handling compression.
    fn decode_packets(data: &Bytes) -> Vec<DecodedPacket> {
        let mut packets = Vec::new();
        let mut offset = 0;

        while offset + 16 <= data.len() {
            let packet_len = BigEndian::read_u32(&data[offset..offset + 4]) as usize;
            let _header_len = BigEndian::read_u16(&data[offset + 4..offset + 6]);
            let version = BigEndian::read_u16(&data[offset + 6..offset + 8]);
            let operation = BigEndian::read_u32(&data[offset + 8..offset + 12]);

            // Every frame has a 16-byte header. Validate the relative length
            // before slicing, and avoid overflowing `offset + packet_len`.
            if packet_len < 16 || packet_len > data.len() - offset {
                break;
            }

            let body = &data[offset + 16..offset + packet_len];

            match version {
                ver::ZLIB => {
                    if let Ok(decompressed) = decompress_zlib(body) {
                        packets.extend(Self::decode_packets(&Bytes::from(decompressed)));
                    }
                }
                ver::BROTLI => {
                    if let Ok(decompressed) = decompress_brotli(body) {
                        packets.extend(Self::decode_packets(&Bytes::from(decompressed)));
                    }
                }
                ver::RAW_JSON | ver::POPULARITY => {
                    packets.push(DecodedPacket {
                        operation,
                        body: data.slice(offset + 16..offset + packet_len),
                    });
                }
                _ => {
                    debug!("Unknown protocol version: {}", version);
                }
            }

            offset += packet_len;
        }

        packets
    }

    /// Commands `parse_notification` turns into items; everything else is
    /// dropped.
    fn handles_command(cmd_base: &str) -> bool {
        matches!(
            cmd_base,
            "DANMU_MSG"
                | "DANMU_MSG_MIRROR"
                | "SEND_GIFT"
                | "SEND_GIFT_V2"
                | "SUPER_CHAT_MESSAGE"
                | "ROOM_CHANGE"
                | "ROOM_LOCK"
                | "CUT_OFF"
        )
    }

    /// Append items from a notification (op=5); V2 gifts can contain a batch.
    fn parse_notification(body: &[u8], items: &mut Vec<DanmuItem>) {
        // Bilibili serialises `cmd` as the first key. Most notifications
        // (INTERACT_WORD, ONLINE_RANK_*, WATCHED_CHANGE, STOP_LIVE_ROOM_LIST,
        // ...) are dropped anyway, so when that layout holds and the command
        // is not handled, skip the frame before it becomes a `Value` tree.
        // Any other layout, or a command with escapes, takes the full parse.
        if let Some(rest) = body.strip_prefix(b"{\"cmd\":\"")
            && let Some(end) = rest.iter().position(|&b| b == b'"')
            && let Ok(cmd) = std::str::from_utf8(&rest[..end])
            && !cmd.contains('\\')
        {
            let cmd_base = cmd.split(':').next().unwrap_or(cmd);
            if !Self::handles_command(cmd_base) {
                return;
            }
            if cmd_base == "SEND_GIFT_V2"
                && let Ok(notification) = serde_json::from_slice::<GiftV2Notification<'_>>(body)
                && notification.cmd.split(':').next() == Some("SEND_GIFT_V2")
            {
                Self::parse_gifts_v2(&notification.data.pb, items);
                return;
            }
            // Duplicate fields or an unexpected envelope use Value's existing
            // semantics below, including keeping the last duplicate field.
        }

        let Ok(json) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        let Some(cmd) = json.get("cmd").and_then(Value::as_str) else {
            return;
        };

        // Handle DANMU_MSG variants (e.g., "DANMU_MSG:4:0:2:2:2:0")
        let cmd_base = cmd.split(':').next().unwrap_or(cmd);
        // DANMU_MSG_MIRROR are mirror of DANMU_MSG

        let item = match cmd_base {
            "DANMU_MSG" | "DANMU_MSG_MIRROR" => {
                Self::parse_danmu_msg(&json).map(DanmuItem::Message)
            }
            "SEND_GIFT" => Self::parse_gift(&json).map(DanmuItem::Message),
            "SEND_GIFT_V2" => {
                if let Some(pb) = json
                    .get("data")
                    .and_then(|data| data.get("pb"))
                    .and_then(Value::as_str)
                {
                    Self::parse_gifts_v2(pb, items);
                }
                return;
            }
            "SUPER_CHAT_MESSAGE" => Self::parse_super_chat(&json).map(DanmuItem::Message),
            "ROOM_CHANGE" => Self::parse_room_change(&json),
            // Stream-ending / enforcement events.
            // Bilibili emits these when the live room is forcibly ended/locked.
            "ROOM_LOCK" | "CUT_OFF" => Self::parse_stream_closed(cmd_base, &json),
            _ => None,
        };
        items.extend(item);
    }

    fn parse_gifts_v2(pb: &str, items: &mut Vec<DanmuItem>) {
        let bytes = match STANDARD.decode(pb) {
            Ok(bytes) => bytes,
            Err(error) => {
                debug!(%error, "Invalid SEND_GIFT_V2 base64 payload");
                return;
            }
        };
        let batch = match SendGiftV2::decode(bytes.as_slice()) {
            Ok(batch) => batch,
            Err(error) => {
                debug!(%error, "Invalid SEND_GIFT_V2 protobuf payload");
                return;
            }
        };
        // Disabled batches must not produce gifts, matching the upstream gate.
        // Record sender identity as delivered: anonymous senders can have uid=0.
        if !batch.switch || batch.gift_list.is_empty() {
            return;
        }

        let gift_count = batch.gift_list.len();
        items.reserve(gift_count);
        let mut user_id = batch.uid.to_string();
        let mut username = batch.uname;
        for (index, gift) in batch.gift_list.into_iter().enumerate() {
            let Ok(count) = u32::try_from(gift.num) else {
                debug!("Ignoring SEND_GIFT_V2 gift with invalid count");
                continue;
            };
            let Ok(price) = u64::try_from(gift.price) else {
                debug!("Ignoring SEND_GIFT_V2 gift with invalid price");
                continue;
            };
            if count == 0
                || gift.gift_name.is_empty()
                || !matches!(gift.coin_type.as_str(), "gold" | "silver")
            {
                debug!("Ignoring SEND_GIFT_V2 gift with invalid name, count or coin type");
                continue;
            }
            let mut message = DanmuMessage::gift(
                uuid::Uuid::new_v4().to_string(),
                // The final gift can own these strings directly; single-gift
                // notifications need no sender-string clones.
                if index + 1 == gift_count {
                    std::mem::take(&mut user_id)
                } else {
                    user_id.clone()
                },
                if index + 1 == gift_count {
                    std::mem::take(&mut username)
                } else {
                    username.clone()
                },
                gift.gift_name,
                count,
            )
            // Preserve the same raw price units as legacy SEND_GIFT.
            .with_metadata("price", serde_json::json!(price));

            // Timestamp belongs to each gift, not the outer JSON envelope.
            // Missing/nonpositive or out-of-range values keep reception time.
            let timestamp = if gift.timestamp > 1_000_000_000_000 {
                Utc.timestamp_millis_opt(gift.timestamp).single()
            } else if gift.timestamp > 0 {
                Utc.timestamp_opt(gift.timestamp, 0).single()
            } else {
                None
            };
            if let Some(timestamp) = timestamp {
                message = message.with_timestamp(timestamp);
            }
            items.push(DanmuItem::Message(message));
        }
    }

    fn parse_stream_closed(cmd: &str, json: &Value) -> Option<DanmuItem> {
        let data = json.get("data");

        let message = data
            .and_then(|d| {
                d.get("message")
                    .or_else(|| d.get("msg"))
                    .or_else(|| d.get("reason"))
                    .or_else(|| d.get("text"))
            })
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| Some(cmd.to_string()));

        Some(DanmuItem::Control(DanmuControlEvent::StreamClosed {
            message,
            action: None,
        }))
    }

    /// Parse ROOM_CHANGE (room info update) into a control event.
    ///
    /// Bilibili sends this when the streamer updates the title / area / tags.  
    fn parse_room_change(json: &Value) -> Option<DanmuItem> {
        let data = json.get("data")?;

        let title = data
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let category = data
            .get("area_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let parent_category = data
            .get("parent_area_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        Some(DanmuItem::Control(DanmuControlEvent::RoomInfoChanged {
            title,
            category,
            parent_category,
        }))
    }

    /// Parse DANMU_MSG into DanmuMessage.
    fn parse_danmu_msg(json: &Value) -> Option<DanmuMessage> {
        let info = json.get("info")?.as_array()?;

        // info[1] = content
        let content = info.get(1)?.as_str()?.to_string();

        // info[2][0] = uid, info[2][1] = name
        let user_info = info.get(2)?.as_array()?;
        let uid = user_info.first()?.as_u64().unwrap_or(0);
        let name = user_info.get(1)?.as_str().unwrap_or("").to_string();

        // info[0][3] = color
        let meta = info.first()?.as_array()?;
        let color = meta
            .get(3)
            .and_then(|v| v.as_u64())
            .map(|c| format!("#{:06X}", c as u32));

        // Check for emoticon in extra field
        let content = if let Some(extra_obj) = meta.get(15) {
            if let Some(extra_str) = extra_obj.get("extra").and_then(|v| v.as_str()) {
                if let Ok(extra) = serde_json::from_str::<Value>(extra_str) {
                    if let Some(emoticon) = extra.get("emoticon_unique").and_then(|v| v.as_str()) {
                        if !emoticon.is_empty() {
                            format!("[表情:{}]", emoticon)
                        } else {
                            content
                        }
                    } else {
                        content
                    }
                } else {
                    content
                }
            } else {
                content
            }
        } else {
            content
        };

        let mut danmu = DanmuMessage::chat(
            uuid::Uuid::new_v4().to_string(),
            uid.to_string(),
            name,
            content,
        );

        if let Some(c) = color {
            danmu = danmu.with_color(c);
        }

        Some(danmu)
    }

    /// Parse SEND_GIFT into DanmuMessage.
    fn parse_gift(json: &Value) -> Option<DanmuMessage> {
        let data = json.get("data")?;

        let name = data.get("uname")?.as_str()?.to_string();
        let uid = data.get("uid")?.as_u64()?;
        let gift_name = data.get("giftName")?.as_str()?.to_string();
        let num = data.get("num").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        let price = data
            .get("price")
            .or_else(|| data.get("total_coin"))
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .unwrap_or(0);

        let timestamp_ms = data
            .get("timestamp")
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
            .map(|ts| {
                if ts > 1_000_000_000_000 {
                    ts
                } else {
                    ts * 1000
                }
            });

        let mut msg = DanmuMessage::gift(
            uuid::Uuid::new_v4().to_string(),
            uid.to_string(),
            name,
            gift_name,
            num,
        )
        .with_metadata("price", serde_json::json!(price));

        if let Some(ts_ms) = timestamp_ms
            && let Some(dt) = Utc.timestamp_millis_opt(ts_ms).single()
        {
            msg = msg.with_timestamp(dt);
        }

        Some(msg)
    }

    /// Parse SUPER_CHAT_MESSAGE into DanmuMessage.
    fn parse_super_chat(json: &Value) -> Option<DanmuMessage> {
        let data = json.get("data")?;

        let user_info = data.get("user_info")?;
        let name = user_info.get("uname")?.as_str()?.to_string();
        let uid = data.get("uid")?.as_u64()?;
        let content = data.get("message")?.as_str()?.to_string();
        let price = data
            .get("price")
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .unwrap_or(0);

        let keep_time = data
            .get("time")
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .unwrap_or(0);

        let timestamp_ms = data
            .get("ts")
            .or_else(|| data.get("timestamp"))
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
            .map(|ts| {
                if ts > 1_000_000_000_000 {
                    ts
                } else {
                    ts * 1000
                }
            });

        let mut msg = DanmuMessage::super_chat(
            uuid::Uuid::new_v4().to_string(),
            uid.to_string(),
            name,
            content,
            price,
        )
        .with_super_chat_keep_time(keep_time);

        if let Some(ts_ms) = timestamp_ms
            && let Some(dt) = Utc.timestamp_millis_opt(ts_ms).single()
        {
            msg = msg.with_timestamp(dt);
        }

        Some(msg)
    }
}

impl DanmuProtocolFactory for BilibiliDanmuProtocol {
    type Protocol = Self;

    fn platform(&self) -> &str {
        "bilibili"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        capture_group_1_owned(&URL_REGEX, url)
    }

    fn create_protocol(&self) -> Self::Protocol {
        Self {
            client: self.client.clone(),
            cookies: self.cookies.clone(),
            uid: None,
            connection_cookies: None,
        }
    }
}

impl DanmuProtocol for BilibiliDanmuProtocol {
    async fn websocket_url(&mut self, room_id: &str) -> Result<String> {
        // First get real room ID
        let real_room_id = self.get_real_room_id(room_id).await?;

        // Then get WebSocket URL (token is retrieved separately for handshake)
        let (ws_url, _token) = self.get_danmu_info(real_room_id).await?;

        Ok(ws_url)
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        ws_headers_origin_referer_ua(
            "https://live.bilibili.com",
            "https://live.bilibili.com",
            DEFAULT_UA,
        )
    }

    fn cookies(&self) -> Option<String> {
        let fallback_buvid3 = generate_fake_buvid3();
        Some(Self::build_cookie_header(
            self.cookies.as_deref(),
            &fallback_buvid3,
        ))
    }

    fn send_cookie_header(&self) -> bool {
        false
    }

    fn normalize_cookies(&self, cookies: &str) -> String {
        let fallback_buvid3 = generate_fake_buvid3();
        Self::build_cookie_header(Some(cookies), &fallback_buvid3)
    }

    fn configure_connection(
        &mut self,
        cookies: Option<&str>,
        _extras: Option<&HashMap<String, String>>,
    ) {
        self.uid = cookies
            .and_then(|cookies| extract_cookie_value(cookies, "DedeUserID"))
            .and_then(|v| v.parse::<u64>().ok());
        self.connection_cookies = cookies.map(ToString::to_string);
    }

    async fn handshake_messages(&mut self, room_id: &str) -> Result<Vec<Message>> {
        // Get real room ID and danmu info
        let real_room_id = self.get_real_room_id(room_id).await?;
        let (_ws_url, token) = self.get_danmu_info(real_room_id).await?;

        // Build auth packet
        let auth_packet = self.build_auth_packet(real_room_id, &token);

        Ok(vec![Message::Binary(auth_packet)])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        Some(Message::Binary(Bytes::from_static(HEARTBEAT)))
    }

    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
    }

    async fn decode_message(
        &mut self,
        message: &Message,
        _room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        match message {
            Message::Binary(data) => {
                let packets = Self::decode_packets(data);
                let mut items = Vec::new();

                for packet in packets {
                    match packet.operation {
                        op::NOTIFICATION => {
                            Self::parse_notification(&packet.body, &mut items);
                        }
                        // op::HEARTBEAT_REPLY => {
                        //     debug!("Bilibili heartbeat reply received");
                        // }
                        op::AUTH_REPLY => {
                            debug!("Bilibili auth reply received");
                        }
                        _ => {
                            // debug!("Unknown operation: {}", packet.operation);
                        }
                    }
                }

                Ok(items.into())
            }
            _ => Ok(DanmuProtocolOutput::default()),
        }
    }
}

/// Build a packet with the given body and operation code.
fn build_packet(body: &[u8], operation: u32) -> Vec<u8> {
    let packet_len = 16 + body.len();
    let mut packet = Vec::with_capacity(packet_len);

    // Header
    packet.extend_from_slice(&(packet_len as u32).to_be_bytes()); // packet length
    packet.extend_from_slice(&16u16.to_be_bytes()); // header length
    packet.extend_from_slice(&1u16.to_be_bytes()); // version
    packet.extend_from_slice(&operation.to_be_bytes()); // operation
    packet.extend_from_slice(&1u32.to_be_bytes()); // sequence

    // Body
    packet.extend_from_slice(body);

    packet
}

/// Decompress zlib data.
fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| DanmakuError::protocol(format!("zlib decompression failed: {}", e)))?;
    Ok(decompressed)
}

/// Decompress brotli data.
fn decompress_brotli(data: &[u8]) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    brotli::BrotliDecompress(&mut std::io::Cursor::new(data), &mut decompressed)
        .map_err(|e| DanmakuError::protocol(format!("brotli decompression failed: {}", e)))?;
    Ok(decompressed)
}

// Re-export the provider type
pub type BilibiliDanmuProvider = WebSocketDanmuProvider<BilibiliDanmuProtocol>;

/// Create a new Bilibili danmu provider.
pub fn create_bilibili_danmu_provider() -> BilibiliDanmuProvider {
    WebSocketDanmuProvider::with_factory(BilibiliDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use crate::danmaku::ConnectionConfig;

    use super::*;

    fn parse_single_notification(body: &[u8]) -> Option<DanmuItem> {
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(body, &mut items);
        assert!(items.len() <= 1);
        items.pop()
    }

    // Independently encoded wire fixture: two gifts, a uid above JS's safe
    // integer range, second/millisecond timestamps, and unknown fields in
    // both the envelope and first gift. Keep literal to detect schema drift.
    const GIFT_V2_PB: &str = "CIGAgICAgIAQEghHaWZ0VXNlclIbEgZSb2NrZXQYBShkQgRnb2xkUIDiz6oGmAZ7UhwSBkZsb3dlchgCKMgBQgZzaWx2ZXJQ+9CV/7wxWAGiBgd1bmtub3du";

    fn gift_v2_body(pb: &str) -> Vec<u8> {
        // cmd first exercises the fast command filter as well as dispatch.
        format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{pb}"}}}}"#).into_bytes()
    }

    fn gift_v2_fixture() -> SendGiftV2 {
        SendGiftV2::decode(STANDARD.decode(GIFT_V2_PB).unwrap().as_slice()).unwrap()
    }

    fn parse_gift_v2_batch(batch: &SendGiftV2) -> Vec<DanmuItem> {
        let body = gift_v2_body(&STANDARD.encode(batch.encode_to_vec()));
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(&body, &mut items);
        items
    }

    #[test]
    #[ignore = "manual decoder microbenchmark; run with --release --ignored --nocapture"]
    fn benchmark_send_gift_v2_notifications() {
        use std::hint::black_box;
        use std::time::Instant;

        for gift_count in [1, 8, 32] {
            let mut batch = gift_v2_fixture();
            batch.gift_list = vec![batch.gift_list[0].clone(); gift_count];
            let body = serde_json::to_vec(&serde_json::json!({
                "cmd": "SEND_GIFT_V2",
                "danmu": {"area": 0},
                "data": {"dmscore": 476, "pb": STANDARD.encode(batch.encode_to_vec())}
            }))
            .unwrap();
            let iterations = 100_000 / gift_count;
            let mut samples = Vec::new();
            for _ in 0..5 {
                let start = Instant::now();
                for _ in 0..iterations {
                    let mut items = Vec::new();
                    BilibiliDanmuProtocol::parse_notification(black_box(&body), &mut items);
                    black_box(items);
                }
                samples.push(start.elapsed().as_nanos() / iterations as u128);
            }
            samples.sort_unstable();
            eprintln!(
                "{gift_count} gifts/notification: median {} ns/notification",
                samples[2]
            );
        }
    }

    fn assert_v2_gifts(items: &[DanmuItem]) {
        assert_eq!(items.len(), 2);
        for (item, name, count, price, timestamp) in [
            (&items[0], "Rocket", 5, 100, 1_700_000_000_000_i64),
            (&items[1], "Flower", 2, 200, 1_700_000_000_123_i64),
        ] {
            let DanmuItem::Message(msg) = item else {
                panic!("expected gift message");
            };
            assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
            assert_eq!(msg.user_id, "9007199254740993");
            assert_eq!(msg.username, "GiftUser");
            assert_eq!(msg.content, format!("赠送 {name} x{count}"));
            assert_eq!(msg.timestamp.timestamp_millis(), timestamp);
            let meta = msg.metadata.as_ref().unwrap();
            assert_eq!(meta["gift_name"], name);
            assert_eq!(meta["gift_count"], count);
            assert_eq!(meta["price"], price);
        }
    }

    #[tokio::test]
    async fn test_send_gift_v2_decodes_batches_in_raw_and_compressed_frames() {
        use std::io::Write;

        let notification = build_packet(&gift_v2_body(GIFT_V2_PB), op::NOTIFICATION);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&notification).unwrap();
        let mut compressed = build_packet(&encoder.finish().unwrap(), op::NOTIFICATION);
        BigEndian::write_u16(&mut compressed[6..8], ver::ZLIB);
        let mut protocol = BilibiliDanmuProtocol::default();
        for frame in [notification, compressed] {
            let output = protocol
                .decode_message(&Message::Binary(frame.into()), "1")
                .await
                .unwrap();
            let (items, outbound) = output.into_parts();
            assert!(outbound.is_empty());
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_accepts_reordered_json_and_command_suffix() {
        let body = format!(r#"{{"data":{{"pb":"{GIFT_V2_PB}"}},"cmd":"SEND_GIFT_V2:1"}}"#);
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(body.as_bytes(), &mut items);
        assert_v2_gifts(&items);
    }

    #[test]
    fn test_send_gift_v2_accepts_escaped_strings_and_duplicate_payload_fields() {
        let escaped_pb = GIFT_V2_PB.replacen('C', r"\u0043", 1);
        for body in [
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{escaped_pb}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_\u00562","data":{{"pb":"{GIFT_V2_PB}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":null,"pb":"{GIFT_V2_PB}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":null,"data":{{"pb":"{GIFT_V2_PB}"}}}}"#),
        ] {
            let mut items = Vec::new();
            BilibiliDanmuProtocol::parse_notification(body.as_bytes(), &mut items);
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_prefix_does_not_override_last_command_or_accept_invalid_json() {
        let body = format!(
            r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{GIFT_V2_PB}","title":"Changed"}},"cmd":"ROOM_CHANGE"}}"#
        );
        let item = parse_single_notification(body.as_bytes()).expect("room change");
        assert!(matches!(
            item,
            DanmuItem::Control(DanmuControlEvent::RoomInfoChanged { title: Some(title), .. })
                if title == "Changed"
        ));

        let mut malformed = gift_v2_body(GIFT_V2_PB);
        malformed.extend_from_slice(b" trailing garbage");
        assert!(parse_single_notification(&malformed).is_none());
    }

    #[test]
    fn test_send_gift_v2_preserves_anonymous_sender_and_skips_unknown_fields() {
        // Independent wire fixture with uid=0, sender_uinfo.anon, and unknown
        // varint/fixed64/length-delimited/fixed32 fields in the batch and gift.
        // Mirrors the anonymous-sender case in BililiveRecorder commit a27640a.
        let pb = "CAASDOWMv+WQjeeUqOaIt1I6EgZGbG93ZXIYAihkQgRnb2xkUMTo6sUGwAz///////////8ByQzvzauJZ0UjAdIMA/8AgN0M776t3lgBelcIABIQCgzljL/lkI3nlKjmiLcgAUpBCAESCWFub25fdGVzdBoCdjEiLEFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9KAHgEgHpEgAAAAAAAAAA8hIA/RIAAAAA";
        let item = parse_single_notification(&gift_v2_body(pb)).expect("anonymous gift");
        let DanmuItem::Message(msg) = item else {
            panic!("expected gift");
        };
        assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
        assert_eq!(msg.user_id, "0");
        assert_eq!(msg.username, "匿名用户");
        assert_eq!(msg.content, "赠送 Flower x2");
        assert_eq!(msg.timestamp.timestamp_millis(), 1_757_066_308_000);
        let meta = msg.metadata.unwrap();
        assert_eq!(meta["gift_count"], 2);
        assert_eq!(meta["price"], 100);
    }

    #[test]
    fn test_send_gift_v2_records_sender_identity_as_delivered() {
        for (uid, uname) in [(0, "匿名用户"), (-1, "TestUser"), (42, "")] {
            let mut batch = gift_v2_fixture();
            batch.uid = uid;
            batch.uname = uname.to_string();
            let items = parse_gift_v2_batch(&batch);
            assert_eq!(items.len(), 2);
            for item in items {
                let DanmuItem::Message(msg) = item else {
                    panic!("expected gift");
                };
                assert_eq!(msg.user_id, uid.to_string());
                assert_eq!(msg.username, uname);
            }
        }
    }

    #[tokio::test]
    async fn test_send_gift_v2_malformed_payloads_do_not_drop_following_notifications() {
        let mut protocol = BilibiliDanmuProtocol::default();
        for data in [
            serde_json::json!({}),
            serde_json::json!({"pb": null}),
            serde_json::json!({"pb": 42}),
            serde_json::json!({"pb": ""}),
            serde_json::json!({"pb": "!invalid!"}),
            // Truncated varint, truncated nested message, wrong wire type.
            serde_json::json!({"pb": STANDARD.encode([0x08, 0x80])}),
            serde_json::json!({"pb": STANDARD.encode([0x52, 0x05, 0x12])}),
            serde_json::json!({"pb": STANDARD.encode([0x0a, 0x00])}),
        ] {
            let body =
                serde_json::to_vec(&serde_json::json!({"cmd": "SEND_GIFT_V2", "data": data}))
                    .unwrap();
            let mut frame = build_packet(&body, op::NOTIFICATION);
            frame.extend(build_packet(&gift_v2_body(GIFT_V2_PB), op::NOTIFICATION));
            let output = protocol
                .decode_message(&Message::Binary(frame.into()), "1")
                .await
                .unwrap();
            let (items, outbound) = output.into_parts();
            assert!(outbound.is_empty());
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_disabled_and_empty_batches_emit_nothing() {
        let mut batch = gift_v2_fixture();
        batch.switch = false;
        assert!(parse_gift_v2_batch(&batch).is_empty());
        batch.switch = true;
        batch.gift_list.clear();
        assert!(parse_gift_v2_batch(&batch).is_empty());
        assert!(parse_gift_v2_batch(&SendGiftV2::default()).is_empty());
    }

    #[test]
    fn test_send_gift_v2_invalid_gifts_do_not_hide_valid_siblings() {
        let fixture = gift_v2_fixture();
        for (count, price, coin_type, name) in [
            (-1, 100, "gold", "Rocket"),
            (i64::MAX, 100, "gold", "Rocket"),
            (0, 100, "gold", "Rocket"),
            (5, -1, "gold", "Rocket"),
            (5, 100, "unknown", "Rocket"),
            (5, 100, "gold", ""),
        ] {
            let mut batch = fixture.clone();
            batch.gift_list[0].num = count;
            batch.gift_list[0].price = price;
            batch.gift_list[0].coin_type = coin_type.to_string();
            batch.gift_list[0].gift_name = name.to_string();
            let items = parse_gift_v2_batch(&batch);
            assert_eq!(items.len(), 1);
            let DanmuItem::Message(msg) = &items[0] else {
                panic!("expected gift");
            };
            assert_eq!(msg.content, "赠送 Flower x2");
        }
    }

    #[test]
    fn test_send_gift_v2_invalid_timestamps_fall_back_to_reception_time() {
        for timestamp in [0, -1, i64::MIN, i64::MAX] {
            let mut batch = gift_v2_fixture();
            batch.gift_list[0].timestamp = timestamp;
            let before = Utc::now();
            let items = parse_gift_v2_batch(&batch);
            let after = Utc::now();
            assert_eq!(items.len(), 2);
            let DanmuItem::Message(msg) = &items[0] else {
                panic!("expected gift");
            };
            assert!(msg.timestamp >= before && msg.timestamp <= after);
        }
    }

    #[test]
    fn test_extract_room_id() {
        let protocol = BilibiliDanmuProtocol::default();

        assert_eq!(
            protocol.extract_room_id("https://live.bilibili.com/12345"),
            Some("12345".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("https://www.bilibili.com/67890"),
            Some("67890".to_string())
        );
    }

    #[test]
    fn test_build_packet() {
        let body = b"test";
        let packet = build_packet(body, op::AUTH);

        assert_eq!(BigEndian::read_u32(&packet[0..4]), 20); // 16 + 4
        assert_eq!(BigEndian::read_u16(&packet[4..6]), 16);
        assert_eq!(BigEndian::read_u16(&packet[6..8]), 1);
        assert_eq!(BigEndian::read_u32(&packet[8..12]), op::AUTH);
        assert_eq!(BigEndian::read_u32(&packet[12..16]), 1);
        assert_eq!(&packet[16..], body);
    }

    #[test]
    fn decode_packets_rejects_lengths_shorter_than_the_header() {
        for packet_len in [0_u32, 15] {
            let mut packet = vec![0_u8; 16];
            BigEndian::write_u32(&mut packet[0..4], packet_len);

            assert!(BilibiliDanmuProtocol::decode_packets(&Bytes::from(packet)).is_empty());
        }
    }

    #[test]
    fn decode_packets_rejects_lengths_beyond_the_frame() {
        let mut packet = vec![0_u8; 16];
        BigEndian::write_u32(&mut packet[0..4], 17);

        assert!(BilibiliDanmuProtocol::decode_packets(&Bytes::from(packet)).is_empty());
    }

    #[test]
    fn test_auth_packet_uses_uid_from_cookies() {
        let mut protocol = BilibiliDanmuProtocol::default();
        protocol.configure_connection(Some("DedeUserID=42; SESSDATA=abc"), None);

        let packet = protocol.build_auth_packet(123, "token");
        let json: serde_json::Value = serde_json::from_slice(&packet[16..]).unwrap();
        assert_eq!(json.get("uid").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(json.get("roomid").and_then(|v| v.as_u64()), Some(123));
    }

    #[test]
    fn test_cookie_buvid3_uses_provided_value() {
        let cookies = "SESSDATA=abc; buvid3=provided; bili_jct=xyz";
        let merged = BilibiliDanmuProtocol::build_cookie_header(Some(cookies), "fallback");
        assert!(merged.contains("buvid3=provided"));
        assert!(!merged.contains("buvid3=fallback"));
        assert!(!merged.contains("buvid3=provided; buvid3="));
        assert!(merged.contains("SESSDATA=abc"));
        assert!(merged.contains("bili_jct=xyz"));
    }

    #[test]
    fn test_cookie_adds_fallback_buvid3_when_missing() {
        let cookies = "SESSDATA=abc; bili_jct=xyz";
        let merged = BilibiliDanmuProtocol::build_cookie_header(Some(cookies), "fallback");
        assert!(merged.starts_with("buvid3=fallback; "));
        assert!(merged.contains("SESSDATA=abc"));
        assert!(merged.contains("bili_jct=xyz"));
    }

    #[test]
    fn test_parse_danmu_msg() {
        let json = serde_json::json!({
            "cmd": "DANMU_MSG",
            "info": [
                [0, 1, 25, 16777215, 0, 0, 0, "", 0, 0, 0, "", 0, "{}", "{}", {"extra": "{}"}],
                "Hello World",
                [12345, "TestUser", 0, 0, 0, 0, 0, ""]
            ]
        });

        let danmu = BilibiliDanmuProtocol::parse_danmu_msg(&json);
        assert!(danmu.is_some());

        let msg = danmu.unwrap();
        assert_eq!(msg.content, "Hello World");
        assert_eq!(msg.username, "TestUser");
        assert_eq!(msg.user_id, "12345");
    }

    #[test]
    fn test_parse_send_gift_emits_gift_message() {
        let json = serde_json::json!({
            "cmd": "SEND_GIFT",
            "data": {
                "uname": "GiftUser",
                "uid": 42,
                "giftName": "Rocket",
                "num": 5,
                "price": 100,
                "timestamp": 1700000000123_i64
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse SEND_GIFT");

        match item {
            DanmuItem::Message(msg) => {
                assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
                assert_eq!(msg.user_id, "42");
                assert_eq!(msg.username, "GiftUser");
                assert_eq!(msg.content, "赠送 Rocket x5");
                let meta = msg.metadata.expect("gift metadata");
                assert_eq!(meta.get("price").unwrap(), 100);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_super_chat_emits_super_chat_message() {
        let json = serde_json::json!({
            "cmd": "SUPER_CHAT_MESSAGE",
            "data": {
                "uid": 99,
                "price": 30,
                "time": 60,
                "ts": 1700000000456_i64,
                "message": "Hello",
                "user_info": {
                    "uname": "SCUser"
                }
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse SUPER_CHAT_MESSAGE");

        match item {
            DanmuItem::Message(msg) => {
                assert_eq!(
                    msg.message_type,
                    crate::danmaku::message::DanmuType::SuperChat
                );
                assert_eq!(msg.user_id, "99");
                assert_eq!(msg.username, "SCUser");
                assert_eq!(msg.content, "Hello");
                let meta = msg.metadata.expect("super chat metadata");
                assert_eq!(meta.get("price").unwrap(), 30);
                assert_eq!(meta.get("keep_time").unwrap(), 60);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_room_change_emits_control() {
        let json = serde_json::json!({
            "cmd": "ROOM_CHANGE",
            "data": {
                "title": "New Stream Title",
                "area_name": "Some Area",
                "parent_area_name": "Some Parent"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse ROOM_CHANGE");

        match item {
            DanmuItem::Control(DanmuControlEvent::RoomInfoChanged {
                title,
                category,
                parent_category,
            }) => {
                assert_eq!(title.as_deref(), Some("New Stream Title"));
                assert_eq!(category.as_deref(), Some("Some Area"));
                assert_eq!(parent_category.as_deref(), Some("Some Parent"));
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_room_lock_emits_stream_closed() {
        let json = serde_json::json!({
            "cmd": "ROOM_LOCK",
            "data": {
                "message": "room locked"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse ROOM_LOCK");

        match item {
            DanmuItem::Control(DanmuControlEvent::StreamClosed { message, action }) => {
                assert_eq!(message.as_deref(), Some("room locked"));
                assert_eq!(action, None);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_cut_off_emits_stream_closed() {
        let json = serde_json::json!({
            "cmd": "CUT_OFF",
            "data": {
                "msg": "cut off"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse CUT_OFF");

        match item {
            DanmuItem::Control(DanmuControlEvent::StreamClosed { message, action }) => {
                assert_eq!(message.as_deref(), Some("cut off"));
                assert_eq!(action, None);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    /// Real integration test - connects to an actual Bilibili live room
    /// Run with: cargo test --package platforms-parser bilibili::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_real_connection() {
        use crate::danmaku::provider::DanmuProvider;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();

        let provider = create_bilibili_danmu_provider();
        let room_id = "1721766859";

        println!("Connecting to Bilibili room: {}", room_id);
        let mut items = match provider.connect(room_id, ConnectionConfig::default()).await {
            Ok(stream) => stream.items,
            Err(e) => {
                eprintln!("Failed to connect: {}", e);
                return;
            }
        };

        // Receive messages for 60 seconds
        let start = std::time::Instant::now();
        let mut message_count = 0;

        while start.elapsed() < Duration::from_secs(60) {
            match tokio::time::timeout(Duration::from_millis(500), items.recv()).await {
                Ok(Some(item)) => match item {
                    crate::danmaku::DanmuItem::Message(msg) => {
                        println!("[{:?}] {}: {}", msg.message_type, msg.username, msg.content);
                        message_count += 1;
                    }
                    crate::danmaku::DanmuItem::Control(control) => {
                        println!("[control] {:?}", control);
                    }
                },
                Ok(None) => {
                    println!("Stream closed by provider");
                    break;
                }
                Err(_) => {
                    // No message within the window; keep waiting until the 60s
                    // budget is spent.
                }
            }
        }

        println!("Received {} messages", message_count);
    }
}
