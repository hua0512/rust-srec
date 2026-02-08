//! Douyin (抖音) danmu provider.
//!
//! Implements danmu collection for the Douyin streaming platform using the generic
//! WebSocket provider with Protobuf protocol for message encoding/decoding.

use async_trait::async_trait;

use bytes::Bytes;
use flate2::read::GzDecoder;
use prost::Message as ProstMessage;
use rustc_hash::FxHashMap;
use std::io::Read;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::ws_headers_origin_ua;
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuControlEvent, DanmuItem, DanmuMessage};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::platforms::douyin::apis::LIVE_DOUYIN_URL;
use crate::extractor::platforms::douyin::douyin_proto;
use crate::extractor::platforms::douyin::generate_xbogus;
use crate::extractor::platforms::douyin::utils::{DEFAULT_TTWID, get_common_params};
use crate::extractor::utils::capture_group_1_owned;
use chrono::{TimeZone, Utc};
use tokio_tungstenite::tungstenite::http::HeaderMap;

use super::URL_REGEX;

/// Douyin WebSocket server hosts
const DOUYIN_WS_HOSTS: &[&str] = &[
    "wss://webcast100-ws-web-lq.douyin.com",
    "wss://webcast100-ws-web-hl.douyin.com",
    "wss://webcast100-ws-web-lf.douyin.com",
];

/// Douyin WebSocket server URL path
const DOUYIN_WS_URL_PATH: &str = "/webcast/im/push/v2/";

/// Heartbeat packet.
const HEARTBEAT: &[u8] = b":\x02hb";
/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Douyin Protocol Implementation
#[derive(Clone, Default)]
pub struct DouyinDanmuProtocol {
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
}

impl DouyinDanmuProtocol {
    /// Create a new DouyinDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new DouyinDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
        }
    }

    /// Generates MD5 hash of the input string, returns 32-byte hex representation
    fn md5_hash(input: &str) -> [u8; 32] {
        use md5::{Digest, Md5};
        let hash = Md5::digest(input.as_bytes());
        let mut result = [0u8; 32];
        // Convert each byte to 2 hex chars
        for (i, byte) in hash.iter().enumerate() {
            let hi = byte >> 4;
            let lo = byte & 0x0f;
            result[i * 2] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
            result[i * 2 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
        }
        result
    }

    /// Creates the WebSocket URL with all required parameters
    fn build_websocket_url(room_id: &str, user_id: &str) -> Result<String> {
        const VERSION_CODE: &str = "180800";
        const WEBCAST_SDK_VERSION: &str = "1.0.15";
        const UPDATE_VERSION_CODE: &str = "1.0.15";

        // Build query params for signature

        let mut query_params = FxHashMap::default();
        // ADD ALL PARAMS FROM get_common_params
        query_params.extend(get_common_params());

        // websocket params
        query_params.insert("version_code", VERSION_CODE);
        query_params.insert("webcast_sdk_version", WEBCAST_SDK_VERSION);
        query_params.insert("update_version_code", UPDATE_VERSION_CODE);
        query_params.insert("host", LIVE_DOUYIN_URL);
        query_params.insert("did_rule", "3");
        query_params.insert("identity", "audience");
        query_params.insert("endpoint", "live_pc");
        query_params.insert("need_persist_msg_count", "15");
        query_params.insert("heartbeatDuration", "0");

        query_params.insert("room_id", room_id);
        query_params.insert("user_unique_id", user_id);

        let query_for_sign = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        let signature_param = format!(
            "live_id=1,aid=6383,version_code={},webcast_sdk_version={},room_id={},sub_room_id=,sub_channel_id=,did_rule=3,user_unique_id={},device_platform=web,device_type=,ac=,identity=audience",
            VERSION_CODE, WEBCAST_SDK_VERSION, room_id, user_id
        );

        let md5_hash = Self::md5_hash(&signature_param);
        // make counter always 1
        let signature_bytes = generate_xbogus(&md5_hash, 1);
        // SAFETY: result contains only ASCII from XBOGUS_ALPHABET
        let signature = unsafe { std::str::from_utf8_unchecked(&signature_bytes) };

        use rand::seq::IndexedRandom;
        let mut rng = rand::rng();
        let host = DOUYIN_WS_HOSTS
            .choose(&mut rng)
            .unwrap_or(&DOUYIN_WS_HOSTS[0]);

        let url = format!(
            "{}{}?{}&signature={}",
            host, DOUYIN_WS_URL_PATH, query_for_sign, signature
        );

        Ok(url)
    }

    /// Generate a random user unique ID.
    fn generate_user_unique_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let base = 7300000000000000000u64;
        let range = 699999999999999999u64;
        // `SystemTime` can be before UNIX_EPOCH on misconfigured systems; default to 0.
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let id = base + (ts % range);
        id.to_string()
    }

    /// Decompresses gzip data
    fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
        let mut decoder = GzDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| DanmakuError::protocol(format!("Failed to decompress gzip: {}", e)))?;
        Ok(decompressed)
    }

    fn create_ack_packet(log_id: u64, internal_ext: Vec<u8>) -> Result<Bytes> {
        let ack_frame = douyin_proto::webcast::im::PushFrame {
            log_id,
            payload_type: "ack".to_string(),
            payload: internal_ext,
            ..Default::default()
        };
        let mut buf = Vec::new();
        ack_frame.encode(&mut buf)?;
        Ok(Bytes::from(buf))
    }

    /// Decodes a PushFrame from raw bytes, handles decompression, and returns the Response and log_id
    fn decode_push_frame(data: &[u8]) -> Result<(douyin_proto::webcast::im::Response, u64)> {
        let push_frame = douyin_proto::webcast::im::PushFrame::decode(data)?;

        // check if push_frame data is gzip encoded
        let compress_type = push_frame
            .headers
            .iter()
            .find(|h| h.key == "compress_type")
            .map(|h| h.value.as_str())
            .unwrap_or("gzip");

        let payload = if compress_type == "gzip" {
            Bytes::from(Self::decompress_gzip(&push_frame.payload)?)
        } else {
            Bytes::from(push_frame.payload)
        };

        // Parse payload Response message
        let response = douyin_proto::webcast::im::Response::decode(payload.as_ref())?;
        Ok((response, push_frame.log_id))
    }

    /// Helper to extract common message fields (id, timestamp, color)
    fn extract_common_info(common: Option<&douyin_proto::webcast::im::Common>) -> (String, u64) {
        let mut timestamp = 0;
        let mut msg_id = String::new();

        if let Some(common) = common {
            msg_id = common.msg_id.to_string();
            timestamp = common.create_time;
        }
        (msg_id, timestamp)
    }

    /// Parses a list of messages from a Response into danmu items.
    fn parse_response_messages(
        messages_list: &[douyin_proto::webcast::im::Message],
    ) -> Vec<DanmuItem> {
        let mut parsed = Vec::new();
        for message in messages_list.iter() {
            #[allow(clippy::single_match)]
            match message.method.as_str() {
                "WebcastChatMessage" => {
                    if let Ok(chat_msg) =
                        douyin_proto::webcast::im::ChatMessage::decode(message.payload.as_ref())
                    {
                        let user = chat_msg.user.as_ref();
                        let user_id = user.map(|u| u.id.to_string()).unwrap_or_default();
                        let username = user.map(|u| u.nickname.clone()).unwrap_or_default();
                        let content = chat_msg.content;
                        let (msg_id, create_time) =
                            Self::extract_common_info(chat_msg.common.as_ref());

                        // Extract color from rtf_content_v2 or rtf_content, fallback to white
                        let color = chat_msg
                            .rtf_content_v2
                            .and_then(|c| c.default_format.map(|f| f.color))
                            .or_else(|| {
                                chat_msg
                                    .rtf_content
                                    .and_then(|c| c.default_format.map(|f| f.color))
                            })
                            .unwrap_or_else(|| "#FFFFFF".to_string());

                        // Timestamp: event_time > common.create_time > current time
                        let timestamp = Some(chat_msg.event_time * 1000)
                            .filter(|&t| t != 0)
                            .or_else(|| Some(create_time).filter(|&t| t != 0))
                            .and_then(|t| Utc.timestamp_millis_opt(t as i64).single())
                            .unwrap_or_else(Utc::now);

                        let danmu = DanmuMessage::chat(msg_id, user_id, username, content)
                            .with_timestamp(timestamp)
                            .with_color(color);
                        parsed.push(DanmuItem::Message(danmu));
                    }
                }
                "WebcastControlMessage" => {
                    if let Ok(control_msg) =
                        douyin_proto::webcast::im::ControlMessage::decode(message.payload.as_ref())
                    {
                        // debug!("Control message: {:?}", control_msg);
                        // Douyin: treat only action == 3 as a stream-closed signal.
                        if control_msg.action == 3 {
                            let tips = control_msg.tips.trim().to_string();
                            let tips = (!tips.is_empty()).then_some(tips);
                            parsed.push(DanmuItem::Control(DanmuControlEvent::StreamClosed {
                                message: tips,
                                action: Some(control_msg.action),
                            }));
                        }
                    }
                }
                _ => {} // _ => debug!("Ignored message type: {}", message.method),
            }
        }
        parsed
    }
}

#[async_trait]
impl DanmuProtocol for DouyinDanmuProtocol {
    fn platform(&self) -> &str {
        "douyin"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        capture_group_1_owned(&URL_REGEX, url)
    }

    async fn websocket_url(&self, room_id: &str) -> Result<String> {
        // Generate a unique user ID for this session
        let user_id = Self::generate_user_unique_id();
        Self::build_websocket_url(room_id, &user_id)
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        ws_headers_origin_ua("https://live.douyin.com", DEFAULT_UA)
    }

    fn cookies(&self) -> Option<String> {
        let base_cookie = format!("ttwid={}", DEFAULT_TTWID);

        // Merge with user-provided cookies
        if let Some(ref user_cookies) = self.cookies {
            Some(format!("{}; {}", base_cookie, user_cookies))
        } else {
            Some(base_cookie)
        }
    }

    async fn handshake_messages(&self, _room_id: &str) -> Result<Vec<Message>> {
        // Douyin doesn't require explicit handshake messages after connection
        // The connection parameters in the URL handle authentication
        Ok(vec![])
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
        tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuItem>> {
        match message {
            Message::Binary(data) => {
                let (response, log_id) = Self::decode_push_frame(data)?;

                if response.need_ack {
                    let ack_packet =
                        Self::create_ack_packet(log_id, response.internal_ext.encode_to_vec())?;
                    tx.send(Message::Binary(ack_packet)).await.map_err(|e| {
                        DanmakuError::connection(format!("Failed to send ack packet: {}", e))
                    })?;
                }

                Ok(Self::parse_response_messages(&response.messages))
            }
            Message::Text(text) => {
                debug!("Received text message: {}", text);
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}

/// Douyin danmu provider type alias.
pub type DouyinDanmuProvider = WebSocketDanmuProvider<DouyinDanmuProtocol>;

/// Creates a new Douyin danmu provider.
pub fn create_douyin_danmu_provider() -> DouyinDanmuProvider {
    WebSocketDanmuProvider::with_protocol(DouyinDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use crate::extractor::platforms::douyin::utils::DEFAULT_TTWID;

    use super::*;

    #[test]
    fn test_gzip_decompression() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        // Create gzip compressed data
        let original = b"Hello, Douyin danmu test data!";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        // Test decompression
        let decompressed = DouyinDanmuProtocol::decompress_gzip(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_create_ack_packet() {
        let log_id = 12345u64;
        let internal_ext = vec![0x01, 0x02, 0x03, 0x04];

        let result = DouyinDanmuProtocol::create_ack_packet(log_id, internal_ext);
        assert!(result.is_ok());

        let packet = result.unwrap();
        assert!(!packet.is_empty());

        // Verify we can decode the packet back
        let decoded = douyin_proto::webcast::im::PushFrame::decode(packet.as_ref());
        assert!(decoded.is_ok());

        let frame = decoded.unwrap();
        assert_eq!(frame.log_id, log_id);
        assert_eq!(frame.payload_type, "ack");
    }

    #[test]
    fn test_platform_name() {
        let protocol = DouyinDanmuProtocol::default();
        assert_eq!(protocol.platform(), "douyin");
    }

    #[test]
    fn test_heartbeat_message() {
        let protocol = DouyinDanmuProtocol::default();
        let heartbeat = protocol.heartbeat_message();
        assert!(heartbeat.is_some());

        // Verify the heartbeat packet bytes: 0x3A 0x02 0x68 0x62 (":\x02hb")
        if let Some(Message::Binary(data)) = heartbeat {
            assert_eq!(data.as_ref(), b":\x02hb");
        } else {
            panic!("Expected binary heartbeat message");
        }
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires JS engine which is slow
    async fn test_websocket_url_generation() {
        let protocol = DouyinDanmuProtocol::default();
        let room_id = "123456789";

        let result = protocol.websocket_url(room_id).await;
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.starts_with("wss://"));
        assert!(url.contains("room_id=123456789"));
        assert!(url.contains("signature="));
        println!("Generated WebSocket URL: {}", url);
    }

    /// Real integration test - connects to an actual Douyin live room
    /// Run with: cargo test --lib douyin::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore] // Requires network access and a live room
    async fn test_real_connection() {
        use futures::StreamExt;
        use tokio_tungstenite::connect_async;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .unwrap();

        let protocol = DouyinDanmuProtocol::default();
        let room_id = "7592278867031231283";
        // Generate WebSocket URL
        let ws_url = protocol
            .websocket_url(room_id)
            .await
            .expect("Failed to generate WebSocket URL");
        println!("Connecting to: {}", ws_url);

        // Build request with required headers
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&ws_url)
            .header("Host", "webcast100-ws-web-lq.douyin.com")
            .header("User-Agent", DEFAULT_UA)
            .header("Origin", "https://live.douyin.com")
            .header("Cookie", format!("ttwid={}", DEFAULT_TTWID))
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .expect("Failed to build request");

        // Connect
        let (mut ws_stream, response) = connect_async(request).await.expect("Failed to connect");

        println!("Connected! Response: {:?}", response.status());

        // Receive a few messages
        let mut message_count = 0;
        let max_messages = 10;
        let timeout = tokio::time::Duration::from_secs(60);

        let result = tokio::time::timeout(timeout, async {
            while let Some(msg_result) = ws_stream.next().await {
                match msg_result {
                    Ok(msg) => {
                        match &msg {
                            Message::Binary(data) => {
                                println!("Received binary message: {} bytes", data.len());

                                // Parse Response using helper
                                match DouyinDanmuProtocol::decode_push_frame(data) {
                                    Ok((response, log_id)) => {
                                        println!(
                                            "  PushFrame log_id: {}, payload_type: msg",
                                            log_id
                                        );
                                        println!(
                                            "  Response: need_ack={}, messages_count={}",
                                            response.need_ack,
                                            response.messages.len()
                                        );

                                        let items = DouyinDanmuProtocol::parse_response_messages(
                                            &response.messages,
                                        );
                                        for item in items {
                                            match item {
                                                DanmuItem::Message(danmu) => println!(
                                                    "    [{}] {}: {}",
                                                    danmu.timestamp, danmu.username, danmu.content
                                                ),
                                                DanmuItem::Control(control) => {
                                                    println!("    [control] {:?}", control);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        println!("  Failed to decode PushFrame: {}", e);
                                    }
                                }

                                message_count += 1;
                                if message_count >= max_messages {
                                    println!("Received {} messages, stopping", max_messages);
                                    break;
                                }
                            }
                            Message::Text(text) => {
                                println!("Received text: {}", text);
                            }
                            Message::Ping(data) => {
                                println!("Received ping: {} bytes", data.len());
                            }
                            Message::Pong(data) => {
                                println!("Received pong: {} bytes", data.len());
                            }
                            Message::Close(frame) => {
                                println!("Received close: {:?}", frame);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        println!("Error receiving message: {}", e);
                        break;
                    }
                }
            }
        })
        .await;

        match result {
            Ok(_) => println!("Test completed successfully"),
            Err(_) => println!("Test timed out after {:?}", timeout),
        }

        assert!(
            message_count > 0,
            "Should have received at least one message"
        );
    }
}
