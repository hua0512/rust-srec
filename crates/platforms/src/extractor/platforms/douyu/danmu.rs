//! Douyu (斗鱼) danmu provider.
//!
//! Implements danmu collection for the Douyu streaming platform using the generic
//! WebSocket provider with STT (Serialized Text Transport) protocol for message encoding/decoding.

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::DanmuMessage;
use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::platforms::douyu::stt;

use super::URL_REGEX;
use super::danmu_models::{
    DouyuChatMessage, DouyuGiftMessage, DouyuMessageType, create_join_group_message,
    create_login_message, parse_message,
};
use super::stt::{create_packet, parse_packets};

/// Douyu WebSocket server URL
const DOUYU_WS_URL: &str = "wss://danmuproxy.douyu.com:8502/";

/// Heartbeat interval in seconds (Douyu requires heartbeat every 45 seconds)
const HEARTBEAT_INTERVAL_SECS: u64 = 45;

/// Default group ID for joining the main danmu group
const DEFAULT_GROUP_ID: i32 = -9999;

/// Douyu Danmu Protocol Implementation
#[derive(Clone, Default)]
pub struct DouyuDanmuProtocol {
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
}

impl DouyuDanmuProtocol {
    /// Create a new DouyuDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new DouyuDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
        }
    }

    /// Parse chat messages from STT payload.
    fn parse_chat_message(map: &rustc_hash::FxHashMap<String, String>) -> Option<DanmuMessage> {
        let chat = DouyuChatMessage::from_map(map)?;

        let mut danmu = DanmuMessage::chat(&chat.id, &chat.uid, &chat.nickname, &chat.content)
            .with_timestamp(Utc::now());

        if let Some(color) = chat.color {
            danmu = danmu.with_color(color);
        }

        // Add metadata
        danmu = danmu
            .with_metadata("level", serde_json::json!(chat.level))
            .with_metadata("room_id", serde_json::json!(chat.room_id));

        if let Some(badge_name) = chat.badge_name {
            danmu = danmu.with_metadata("badge_name", serde_json::json!(badge_name));
        }
        if let Some(badge_level) = chat.badge_level {
            danmu = danmu.with_metadata("badge_level", serde_json::json!(badge_level));
        }
        if let Some(platform) = chat.platform {
            danmu = danmu.with_metadata("platform", serde_json::json!(platform));
        }
        if let Some(noble_level) = chat.noble_level {
            danmu = danmu.with_metadata("noble_level", serde_json::json!(noble_level));
        }

        Some(danmu)
    }

    /// Parse gift messages from STT payload.
    fn parse_gift_message(map: &rustc_hash::FxHashMap<String, String>) -> Option<DanmuMessage> {
        let gift = DouyuGiftMessage::from_map(map)?;

        let danmu = DanmuMessage::gift(
            &gift.gift_id,
            &gift.uid,
            &gift.nickname,
            &gift.gift_name,
            gift.gift_count,
        )
        .with_timestamp(Utc::now())
        .with_metadata("room_id", serde_json::json!(gift.room_id))
        .with_metadata("hits", serde_json::json!(gift.hits));

        Some(danmu)
    }
}

#[async_trait]
impl DanmuProtocol for DouyuDanmuProtocol {
    fn platform(&self) -> &str {
        "douyu"
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

    async fn websocket_url(&self, _room_id: &str) -> Result<String> {
        Ok(DOUYU_WS_URL.to_string())
    }

    fn headers(&self, _room_id: &str) -> Vec<(String, String)> {
        vec![
            ("Origin".to_string(), "https://www.douyu.com".to_string()),
            ("Referer".to_string(), "https://www.douyu.com".to_string()),
            ("User-Agent".to_string(), DEFAULT_UA.to_string()),
        ]
    }

    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    async fn handshake_messages(&self, room_id: &str) -> Result<Vec<Message>> {
        // Create login and join group messages
        let login_msg = create_login_message(room_id);
        let join_group_msg = create_join_group_message(room_id, DEFAULT_GROUP_ID);

        debug!("Douyu login message: {}", login_msg);
        debug!("Douyu join group message: {}", join_group_msg);

        // Create binary packets
        let login_packet = create_packet(&login_msg);
        let join_group_packet = create_packet(&join_group_msg);

        Ok(vec![
            Message::Binary(login_packet),
            Message::Binary(join_group_packet),
        ])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        Some(Message::Binary(Bytes::from(stt::HEARTBEAT)))
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
                let packets = parse_packets(data);
                let mut danmus = Vec::new();

                for payload in packets {
                    let (msg_type, map) = parse_message(&payload);

                    match msg_type {
                        DouyuMessageType::ChatMsg => {
                            if let Some(danmu) = Self::parse_chat_message(&map) {
                                danmus.push(danmu);
                            }
                        }
                        DouyuMessageType::Gift => {
                            if let Some(danmu) = Self::parse_gift_message(&map) {
                                danmus.push(danmu);
                            }
                        }
                        DouyuMessageType::LoginRes => {
                            debug!("Douyu login response received");
                        }
                        DouyuMessageType::KeepAlive => {
                            debug!("Douyu keepalive received");
                        }
                        DouyuMessageType::UserEnter => {
                            // Optionally handle user enter events
                            debug!("User entered room: {:?}", map.get("nn"));
                        }
                        _ => {
                            // Ignore other message types
                        }
                    }
                }

                Ok(danmus)
            }
            Message::Text(text) => {
                debug!("Received text message: {}", text);
                Ok(vec![])
            }
            Message::Ping(_) => {
                debug!("Received ping");
                Ok(vec![])
            }
            Message::Pong(_) => {
                debug!("Received pong");
                Ok(vec![])
            }
            Message::Close(frame) => {
                debug!("Received close frame: {:?}", frame);
                Err(DanmakuError::connection("Connection closed by server"))
            }
            _ => Ok(vec![]),
        }
    }
}

/// Douyu danmu provider type alias.
pub type DouyuDanmuProvider = WebSocketDanmuProvider<DouyuDanmuProtocol>;

/// Creates a new Douyu danmu provider.
pub fn create_douyu_danmu_provider() -> DouyuDanmuProvider {
    WebSocketDanmuProvider::with_protocol(DouyuDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{danmaku::provider::DanmuProvider, extractor::default::DEFAULT_UA};

    #[test]
    fn test_url_matching() {
        let protocol = DouyuDanmuProtocol::default();

        // Test supported URLs
        assert!(protocol.supports_url("https://www.douyu.com/123456"));
        assert!(protocol.supports_url("http://douyu.com/789012"));
        assert!(protocol.supports_url("https://douyu.com/1234567890"));

        // Test unsupported URLs
        assert!(!protocol.supports_url("https://www.huya.com/123"));
        assert!(!protocol.supports_url("https://www.bilibili.com/123"));
        assert!(!protocol.supports_url("https://douyu.com/")); // No room ID
    }

    #[test]
    fn test_extract_room_id() {
        let protocol = DouyuDanmuProtocol::default();

        assert_eq!(
            protocol.extract_room_id("https://www.douyu.com/123456"),
            Some("123456".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("http://douyu.com/789012"),
            Some("789012".to_string())
        );
        assert_eq!(protocol.extract_room_id("https://www.huya.com/123"), None);
    }

    #[test]
    fn test_platform_name() {
        let protocol = DouyuDanmuProtocol::default();
        assert_eq!(protocol.platform(), "douyu");
    }

    #[test]
    fn test_heartbeat_message() {
        let protocol = DouyuDanmuProtocol::default();
        let heartbeat = protocol.heartbeat_message();
        assert!(heartbeat.is_some());

        if let Some(Message::Binary(data)) = heartbeat {
            assert!(!data.is_empty());
        } else {
            panic!("Expected binary heartbeat message");
        }
    }

    #[test]
    fn test_heartbeat_interval() {
        let protocol = DouyuDanmuProtocol::default();
        assert_eq!(protocol.heartbeat_interval(), Duration::from_secs(45));
    }

    #[tokio::test]
    async fn test_handshake_messages() {
        let protocol = DouyuDanmuProtocol::default();
        let messages = protocol.handshake_messages("123456").await.unwrap();

        assert_eq!(messages.len(), 2); // Login + JoinGroup

        for msg in messages {
            match msg {
                Message::Binary(data) => {
                    assert!(!data.is_empty());
                }
                _ => panic!("Expected binary messages"),
            }
        }
    }

    #[tokio::test]
    async fn test_websocket_url() {
        let protocol = DouyuDanmuProtocol::default();
        let url = protocol.websocket_url("123456").await.unwrap();

        assert!(url.starts_with("wss://"));
        assert!(url.contains("douyu.com"));
    }

    #[test]
    fn test_parse_chat_message() {
        use super::super::stt::stt_decode;

        let payload = "type@=chatmsg/rid@=123456/uid@=user123/nn@=TestUser/txt@=Hello World!/level@=10/cid@=msg001/col@=0/";
        let map = stt_decode(payload);

        let danmu = DouyuDanmuProtocol::parse_chat_message(&map).unwrap();

        assert_eq!(danmu.id, "msg001");
        assert_eq!(danmu.user_id, "user123");
        assert_eq!(danmu.username, "TestUser");
        assert_eq!(danmu.content, "Hello World!");
    }

    #[test]
    fn test_parse_gift_message() {
        use super::super::stt::stt_decode;

        let payload = "type@=dgb/rid@=123456/uid@=user123/nn@=GiftUser/gfid@=gift001/gfname@=Rocket/gfcnt@=5/hits@=10/";
        let map = stt_decode(payload);

        let danmu = DouyuDanmuProtocol::parse_gift_message(&map).unwrap();

        assert_eq!(danmu.user_id, "user123");
        assert_eq!(danmu.username, "GiftUser");

        let metadata = danmu.metadata.as_ref().unwrap();
        assert_eq!(metadata.get("gift_name").unwrap(), "Rocket");
        assert_eq!(metadata.get("gift_count").unwrap(), 5);
    }

    /// Real integration test - connects to an actual Douyu live room
    /// Run with: cargo test --package platforms douyu::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore] // Requires network access and a live room
    async fn test_real_connection() {
        use futures::StreamExt;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::http::Request;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();

        let protocol = DouyuDanmuProtocol::default();
        let room_id = "286993";

        // Get WebSocket URL
        let ws_url = protocol.websocket_url(room_id).await.unwrap();
        println!("Connecting to: {}", ws_url);

        // Build request with required headers
        let request = Request::builder()
            .uri(&ws_url)
            .header("Host", "danmuproxy.douyu.com:8502")
            .header("Origin", "https://www.douyu.com")
            .header("Referer", "https://www.douyu.com")
            .header("User-Agent", DEFAULT_UA)
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

        // Send handshake messages
        use futures::SinkExt;
        let handshake_msgs = protocol.handshake_messages(room_id).await.unwrap();
        for msg in handshake_msgs {
            ws_stream.send(msg).await.expect("Failed to send handshake");
        }
        println!("Handshake sent");

        // Create channel for responses (not used in this test)
        let (tx, _rx) = mpsc::channel(100);

        // Receive messages
        let mut message_count = 0;
        let max_messages = 20;
        let timeout = tokio::time::Duration::from_secs(60);

        let result = tokio::time::timeout(timeout, async {
            while let Some(msg_result) = ws_stream.next().await {
                match msg_result {
                    Ok(msg) => match protocol.decode_message(&msg, room_id, &tx).await {
                        Ok(danmus) => {
                            for danmu in danmus {
                                println!(
                                    "[{}] {}: {}",
                                    danmu.timestamp.format("%H:%M:%S"),
                                    danmu.username,
                                    danmu.content
                                );
                                message_count += 1;
                            }
                        }
                        Err(e) => {
                            if e.to_string().contains("closed") {
                                break;
                            }
                            println!("Decode error: {}", e);
                        }
                    },
                    Err(e) => {
                        println!("WebSocket error: {}", e);
                        break;
                    }
                }

                if message_count >= max_messages {
                    println!("Received {} messages, stopping", max_messages);
                    break;
                }

                // Send heartbeat periodically (simplified for test)
                if message_count % 5 == 0 {
                    if let Some(heartbeat) = protocol.heartbeat_message() {
                        let _ = ws_stream.send(heartbeat).await;
                    }
                }
            }
        })
        .await;

        match result {
            Ok(_) => println!("Test completed successfully"),
            Err(_) => println!("Test timed out after {:?}", timeout),
        }

        println!("Total messages received: {}", message_count);
    }
}
