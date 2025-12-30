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
use std::io::Read;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::DanmuMessage;
use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::extractor::default::{DEFAULT_UA, default_client};

use super::URL_REGEX;
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
        }
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
        let response = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://live.bilibili.com")
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
    fn build_auth_packet(room_id: u64, token: &str) -> Bytes {
        let auth_data = AuthData {
            uid: 0, // Anonymous
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

    /// Parse a notification message (op=5) into DanmuMessage.
    fn parse_notification(body: &[u8]) -> Option<DanmuMessage> {
        let json: Value = serde_json::from_slice(body).ok()?;
        let cmd = json.get("cmd")?.as_str()?;

        // Handle DANMU_MSG variants (e.g., "DANMU_MSG:4:0:2:2:2:0")
        let cmd_base = cmd.split(':').next().unwrap_or(cmd);
        // DANMU_MSG_MIRROR are mirror of DANMU_MSG

        match cmd_base {
            "DANMU_MSG" | "DANMU_MSG_MIRROR" => Self::parse_danmu_msg(&json),
            "SEND_GIFT" => Self::parse_gift(&json),
            "SUPER_CHAT_MESSAGE" => Self::parse_super_chat(&json),
            _ => None,
        }
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

        Some(DanmuMessage::gift(
            uuid::Uuid::new_v4().to_string(),
            uid.to_string(),
            name,
            gift_name,
            num,
        ))
    }

    /// Parse SUPER_CHAT_MESSAGE into DanmuMessage.
    fn parse_super_chat(json: &Value) -> Option<DanmuMessage> {
        let data = json.get("data")?;

        let user_info = data.get("user_info")?;
        let name = user_info.get("uname")?.as_str()?.to_string();
        let uid = data.get("uid")?.as_u64()?;
        let content = data.get("message")?.as_str()?.to_string();
        let price = data.get("price").and_then(|v| v.as_u64()).unwrap_or(0);

        // Use chat() with metadata for super chat since super_chat() doesn't exist
        let mut danmu = DanmuMessage::chat(
            uuid::Uuid::new_v4().to_string(),
            uid.to_string(),
            name,
            format!("[SC ￥{}] {}", price, content),
        );

        danmu = danmu.with_metadata("type", "super_chat".into());
        danmu = danmu.with_metadata("price", (price * 1000).into());

        Some(danmu)
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
        let buvid3 = generate_fake_buvid3();
        let base_cookie = format!("buvid3={}", buvid3);

        // Merge with user-provided cookies
        if let Some(ref user_cookies) = self.cookies {
            Some(format!("{}; {}", base_cookie, user_cookies))
        } else {
            Some(base_cookie)
        }
    }

    async fn handshake_messages(&self, room_id: &str) -> Result<Vec<Message>> {
        // Get real room ID and danmu info
        let real_room_id = self.get_real_room_id(room_id).await?;
        let (_ws_url, token) = self.get_danmu_info(real_room_id).await?;

        // Build auth packet
        let auth_packet = Self::build_auth_packet(real_room_id, &token);

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
    ) -> Result<Vec<DanmuMessage>> {
        match message {
            Message::Binary(data) => {
                let packets = Self::decode_packets(data);
                let mut danmus = Vec::new();

                for packet in packets {
                    match packet.operation {
                        op::NOTIFICATION => {
                            if let Some(danmu) = Self::parse_notification(&packet.body) {
                                danmus.push(danmu);
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

                Ok(danmus)
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
                Ok(Some(msg)) => {
                    println!("[{:?}] {}: {}", msg.message_type, msg.username, msg.content);
                    message_count += 1;
                }
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
