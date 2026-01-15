//! Bilibili (哔哩哔哩) danmu provider.
//!
//! Implements danmu collection for Bilibili live streaming using the generic
//! WebSocket provider with binary protocol and Brotli/Zlib compression.

use async_trait::async_trait;
use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;
use flate2::read::ZlibDecoder;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuControlEvent, DanmuItem, DanmuMessage};
use crate::extractor::default::{DEFAULT_UA, default_client};
use chrono::{TimeZone, Utc};

use super::URL_REGEX;
use super::cookie_utils::{extract_cookie_value, strip_refresh_token};
use super::utils::generate_fake_buvid3;
use super::wbi::{encode_wbi, get_wbi_keys};

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
#[allow(dead_code)]
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
    body: Vec<u8>,
}

/// Bilibili Danmu Protocol Implementation
#[derive(Clone, Default)]
pub struct BilibiliDanmuProtocol {
    client: Client,
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
    uid: Option<u64>,
    connection_cookies: Option<String>,
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

        let json_data = serde_json::to_vec(&auth_data).unwrap();
        Bytes::from(build_packet(&json_data, op::AUTH))
    }

    /// Decode packets, handling compression.
    fn decode_packets(data: &[u8]) -> Vec<DecodedPacket> {
        let mut packets = Vec::new();
        let mut offset = 0;

        while offset + 16 <= data.len() {
            let packet_len = BigEndian::read_u32(&data[offset..offset + 4]) as usize;
            let _header_len = BigEndian::read_u16(&data[offset + 4..offset + 6]);
            let version = BigEndian::read_u16(&data[offset + 6..offset + 8]);
            let operation = BigEndian::read_u32(&data[offset + 8..offset + 12]);

            if offset + packet_len > data.len() {
                break;
            }

            let body = &data[offset + 16..offset + packet_len];

            match version {
                ver::ZLIB => {
                    if let Ok(decompressed) = decompress_zlib(body) {
                        packets.extend(Self::decode_packets(&decompressed));
                    }
                }
                ver::BROTLI => {
                    if let Ok(decompressed) = decompress_brotli(body) {
                        packets.extend(Self::decode_packets(&decompressed));
                    }
                }
                ver::RAW_JSON | ver::POPULARITY => {
                    packets.push(DecodedPacket {
                        operation,
                        body: body.to_vec(),
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

    /// Parse a notification message (op=5) into a danmu item.
    fn parse_notification(body: &[u8]) -> Option<DanmuItem> {
        let json: Value = serde_json::from_slice(body).ok()?;
        let cmd = json.get("cmd")?.as_str()?;

        // Handle DANMU_MSG variants (e.g., "DANMU_MSG:4:0:2:2:2:0")
        let cmd_base = cmd.split(':').next().unwrap_or(cmd);
        // DANMU_MSG_MIRROR are mirror of DANMU_MSG

        match cmd_base {
            "DANMU_MSG" | "DANMU_MSG_MIRROR" => {
                Self::parse_danmu_msg(&json).map(DanmuItem::Message)
            }
            "SEND_GIFT" => Self::parse_gift(&json).map(DanmuItem::Message),
            "SUPER_CHAT_MESSAGE" => Self::parse_super_chat(&json).map(DanmuItem::Message),
            "ROOM_CHANGE" => Self::parse_room_change(&json),
            // Stream-ending / enforcement events.
            // Bilibili emits these when the live room is forcibly ended/locked.
            "ROOM_LOCK" | "CUT_OFF" => Self::parse_stream_closed(cmd_base, &json),
            _ => None,
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

#[async_trait]
impl DanmuProtocol for BilibiliDanmuProtocol {
    fn platform(&self) -> &str {
        "bilibili"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        URL_REGEX
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    async fn websocket_url(&self, room_id: &str) -> Result<String> {
        // First get real room ID
        let real_room_id = self.get_real_room_id(room_id).await?;

        // Then get WebSocket URL (token is retrieved separately for handshake)
        let (ws_url, _token) = self.get_danmu_info(real_room_id).await?;

        Ok(ws_url)
    }

    fn headers(&self, _room_id: &str) -> Vec<(String, String)> {
        vec![
            ("User-Agent".to_string(), DEFAULT_UA.to_string()),
            (
                "Origin".to_string(),
                "https://live.bilibili.com".to_string(),
            ),
            (
                "Referer".to_string(),
                "https://live.bilibili.com".to_string(),
            ),
        ]
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

    async fn handshake_messages(&self, room_id: &str) -> Result<Vec<Message>> {
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
        &self,
        message: &Message,
        _room_id: &str,
        _tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuItem>> {
        match message {
            Message::Binary(data) => {
                let packets = Self::decode_packets(data);
                let mut items = Vec::new();

                for packet in packets {
                    match packet.operation {
                        op::NOTIFICATION => {
                            if let Some(item) = Self::parse_notification(&packet.body) {
                                items.push(item);
                            }
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

                Ok(items)
            }
            _ => Ok(vec![]),
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
    WebSocketDanmuProvider::with_protocol(BilibiliDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use crate::danmaku::ConnectionConfig;

    use super::*;

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
        let item =
            BilibiliDanmuProtocol::parse_notification(&body).expect("should parse SEND_GIFT");

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
        let item = BilibiliDanmuProtocol::parse_notification(&body)
            .expect("should parse SUPER_CHAT_MESSAGE");

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
        let item =
            BilibiliDanmuProtocol::parse_notification(&body).expect("should parse ROOM_CHANGE");

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
        let item =
            BilibiliDanmuProtocol::parse_notification(&body).expect("should parse ROOM_LOCK");

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
        let item = BilibiliDanmuProtocol::parse_notification(&body).expect("should parse CUT_OFF");

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
        let connection = match provider.connect(room_id, ConnectionConfig::default()).await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("Failed to connect: {}", e);
                return;
            }
        };

        // Receive messages for 60 seconds
        let start = std::time::Instant::now();
        let mut message_count = 0;

        while start.elapsed() < Duration::from_secs(60) {
            match provider.receive(&connection).await {
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
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    println!("Error: {}", e);
                    break;
                }
            }
        }

        println!("Received {} messages", message_count);
    }
}
