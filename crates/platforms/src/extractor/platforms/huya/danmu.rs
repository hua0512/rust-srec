//! Huya (虎牙) danmu provider.
//!
//! Implements danmu collection for the Huya streaming platform using the generic
//! WebSocket provider with TARS protocol for message encoding/decoding.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use rustc_hash::FxHashMap;
use std::time::Duration;
use tars_codec::{
    decode_tars_struct, encode_tars_value, next_request_id,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::ConnectionConfig;
use crate::danmaku::DanmuMessage;
use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::extractor::platforms::huya::huya_uri;
use crate::extractor::platforms::huya::{HuyaSourceType, HuyaWsCmd};
use crate::extractor::platforms::huya::{
    HuyaUserId, LiveAppUAEx, LiveLaunchReq, LiveUserBase, MessageNotice, WebSocketCommand,
    WsPushMessage, WsRegisterGroupReq, build_get_living_info_request,
};

use super::URL_REGEX;

/// Huya WebSocket server URL
const HUYA_WS_URL: &str = "wss://cdnws.api.huya.com:443";

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 60;

// Heartbeat message
const HEARTBEAT: &[u8] = &[
    0x00, 0x03, 0x1d, 0x00, 0x00, 0x69, 0x00, 0x00, 0x00, 0x69, 0x10, 0x03, 0x2c, 0x3c, 0x4c, 0x56,
    0x08, 0x6f, 0x6e, 0x6c, 0x69, 0x6e, 0x65, 0x75, 0x69, 0x66, 0x0f, 0x4f, 0x6e, 0x55, 0x73, 0x65,
    0x72, 0x48, 0x65, 0x61, 0x72, 0x74, 0x42, 0x65, 0x61, 0x74, 0x7d, 0x00, 0x00, 0x3c, 0x08, 0x00,
    0x01, 0x06, 0x04, 0x74, 0x52, 0x65, 0x71, 0x1d, 0x00, 0x00, 0x2f, 0x0a, 0x0a, 0x0c, 0x16, 0x00,
    0x26, 0x00, 0x36, 0x07, 0x61, 0x64, 0x72, 0x5f, 0x77, 0x61, 0x70, 0x46, 0x00, 0x0b, 0x12, 0x03,
    0xae, 0xf0, 0x0f, 0x22, 0x03, 0xae, 0xf0, 0x0f, 0x3c, 0x42, 0x6d, 0x52, 0x02, 0x60, 0x5c, 0x60,
    0x01, 0x7c, 0x82, 0x00, 0x0b, 0xb0, 0x1f, 0x9c, 0xac, 0x0b, 0x8c, 0x98, 0x0c, 0xa8, 0x0c, 0x20,
];

/// Huya Protocol Implementation
#[derive(Clone, Default)]
pub struct HuyaDanmuProtocol {
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
}

impl HuyaDanmuProtocol {
    /// Create a new HuyaDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new HuyaDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
        }
    }
}

#[async_trait]
impl DanmuProtocol for HuyaDanmuProtocol {
    fn platform(&self) -> &str {
        "huya"
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
        Ok(HUYA_WS_URL.to_string())
    }

    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    async fn handshake_messages(&self, room_id: &str) -> Result<Vec<Message>> {
        let room_id_num: i64 = room_id
            .parse()
            .map_err(|_| DanmakuError::connection("Invalid room ID".to_string()))?;

        // 2512200523
        let ua = format!("webh5&{}&websocket", Utc::now().format("%y%m%d%H%M"));
        // let ua = "webh5&2512200523&websocket";
        debug!("ua: {}", ua);
        let device = "chrome";

        // 1. Living info packet
        let living_info_packet =
            HuyaDanmuProtocol::create_living_info_packet(room_id_num, &ua, &device)?;

        // 2. Register packet
        let reg_packet = HuyaDanmuProtocol::create_do_launch_packet(&ua, &device)?;

        Ok(vec![
            Message::Binary(living_info_packet),
            Message::Binary(reg_packet),
        ])
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
        room_id: &str,
        tx: &tokio::sync::mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuMessage>> {
        match message {
            Message::Binary(data) => self.parse_socket_command(data, room_id, tx).await,
            _ => Ok(vec![]),
        }
    }
}

// Re-export the provider type
pub type HuyaDanmuProvider = WebSocketDanmuProvider<HuyaDanmuProtocol>;

impl HuyaDanmuProvider {
    pub fn new() -> Self {
        WebSocketDanmuProvider::with_protocol(HuyaDanmuProtocol::default(), None)
    }
}

// Static helper methods for TARS encoding/decoding
impl HuyaDanmuProtocol {
    pub fn create_living_info_packet(presenter_uid: i64, ua: &str, device: &str) -> Result<Bytes> {
        let packet = build_get_living_info_request(presenter_uid, ua, device)
            .map_err(|e| DanmakuError::connection(format!("Failed to build packet: {}", e)))?;
        // wrap into socket command
        let req = WebSocketCommand::new(
            HuyaWsCmd::RegisterReq as i32,
            packet.to_vec(),
            0,
            "".to_string(),
            0,
            0,
            "".to_string(),
        );

        // encode socket command into bytes
        let req_bytes = encode_tars_value(&TarsValue::from(req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode packet: {}", e)))?
            .freeze();

        Ok(req_bytes)
    }

    pub fn create_do_launch_packet(ua: &str, device: &str) -> Result<Bytes> {
        let user_id = HuyaUserId::new(
            0,
            String::new(),
            String::new(),
            ua.to_string(),
            String::new(),
            0,
            device.to_string(),
            String::new(),
        );
        let user_base = LiveUserBase::new(HuyaSourceType::PcWeb as i32, 0, LiveAppUAEx::default());
        let live_launch_req = LiveLaunchReq::new(user_id, user_base, true);

        let body_bytes = encode_tars_value(&TarsValue::from(live_launch_req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode body: {}", e)))?
            .freeze();

        let mut body_map = FxHashMap::default();
        body_map.insert("tReq".to_string(), body_bytes);

        let message = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: next_request_id(),
                servant_name: "liveui".to_string(),
                func_name: "doLaunch".to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: body_map,
        };

        let encoded = tars_codec::encode_request(&message).map_err(|e| {
            DanmakuError::connection(format!("Failed to encode room join packet: {}", e))
        })?;

        let socket_cmd = WebSocketCommand::new(
            HuyaWsCmd::RegisterReq as i32,
            encoded.to_vec(),
            0,
            "".to_string(),
            0,
            0,
            "".to_string(),
        );

        let socket_bytes = encode_tars_value(&TarsValue::from(socket_cmd))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode socket cmd: {}", e)))?
            .freeze();

        Ok(socket_bytes)
    }

    pub fn create_create_join_group_packet(presenter_uid: i64) -> Result<Bytes> {
        let register_req = WsRegisterGroupReq::new(
            vec![
                format!("live:{}", presenter_uid),
                format!("chat:{}", presenter_uid),
            ],
            String::new(), // token is empty
        );
        let register_vec = encode_tars_value(&TarsValue::from(register_req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode register: {}", e)))?
            .to_vec();
        let socket_cmd = WebSocketCommand::new(
            HuyaWsCmd::RegisterGroupReq as i32,
            register_vec,
            0,
            "".to_string(),
            0,
            0,
            "".to_string(),
        );
        Ok(encode_tars_value(&TarsValue::from(socket_cmd))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode socket cmd: {}", e)))?
            .freeze())
    }

    /// Parse incoming WebSocket command and handle protocol-level responses
    async fn parse_socket_command(
        &self,
        data: &[u8],
        room_id: &str,
        tx: &tokio::sync::mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuMessage>> {
        if data.len() < 4 {
            return Ok(vec![]);
        }

        // First, decode the WebSocketCommand wrapper (naked struct)
        let tars_val = match decode_tars_struct(Bytes::copy_from_slice(data)) {
            Ok(v) => v,
            Err(e) => {
                debug!("Failed to decode TARS value from socket command: {}", e);
                return Ok(vec![]);
            }
        };

        let socket_cmd = match WebSocketCommand::try_from(tars_val) {
            Ok(cmd) => cmd,
            Err(e) => {
                debug!("Failed to convert TARS value to WebSocketCommand: {}", e);
                return Ok(vec![]);
            }
        };

        // Check the command type
        let cmd_type = socket_cmd.cmd_type();
        match cmd_type {
            // HeartbeatRsp (2) - just acknowledge
            2 => {
                debug!("Received heartbeat response");
                Ok(vec![])
            }
            // RegisterRsp (4) - check if it's doLaunch response, then send join group packet
            4 => {
                let inner_data = socket_cmd.data();
                if inner_data.is_empty() {
                    debug!("Received empty RegisterRsp");
                    return Ok(vec![]);
                }

                // Decode the inner TARS message to check func_name
                let mut src = BytesMut::from(inner_data.as_slice());
                let message = match tars_codec::decode_response(&mut src) {
                    Ok(Some(msg)) => msg,
                    Ok(None) => {
                        debug!("No TARS message in RegisterRsp");
                        return Ok(vec![]);
                    }
                    Err(e) => {
                        debug!("Failed to decode RegisterRsp inner message: {}", e);
                        return Ok(vec![]);
                    }
                };

                // Only send join group packet for doLaunch response
                if message.header.func_name == "doLaunch" {
                    debug!("Received doLaunch response, sending join group packet");
                    if let Ok(presenter_uid) = room_id.parse::<i64>() {
                        match Self::create_create_join_group_packet(presenter_uid) {
                            Ok(packet) => {
                                if let Err(e) = tx.send(Message::Binary(packet)).await {
                                    debug!("Failed to send join group packet: {}", e);
                                } else {
                                    debug!(
                                        "Sent join group packet for presenter_uid: {}",
                                        presenter_uid
                                    );
                                }
                            }
                            Err(e) => {
                                debug!("Failed to create join group packet: {}", e);
                            }
                        }
                    } else {
                        debug!("Invalid room_id for join group: {}", room_id);
                    }
                } else {
                    debug!(
                        "Received RegisterRsp with func_name: {}",
                        message.header.func_name
                    );
                }
                Ok(vec![])
            }
            // WupRsp (7) - Push messages in WsPushMessage format
            7 => {
                let inner_data = socket_cmd.data();
                if inner_data.is_empty() {
                    return Ok(vec![]);
                }

                let push_msg_val = match decode_tars_struct(Bytes::copy_from_slice(inner_data)) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("Failed to decode WsPushMessage TARS: {}", e);
                        return Ok(vec![]);
                    }
                };

                match WsPushMessage::try_from(push_msg_val) {
                    Ok(push_msg) => {
                        // we only want 1400 uri (danmu messages)
                        if push_msg.i_uri != huya_uri::MESSAGE_NOTICE as i64 {
                            return Ok(vec![]);
                        }

                        // Parse the inner s_msg data as MessageNotice
                        if !push_msg.s_msg.is_empty() {
                            let notice_val =
                                match decode_tars_struct(Bytes::copy_from_slice(&push_msg.s_msg)) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        debug!("Failed to decode MessageNotice TARS: {}", e);
                                        return Ok(vec![]);
                                    }
                                };

                            match MessageNotice::try_from(notice_val) {
                                Ok(danmu_content) => {
                                    let username = &danmu_content.t_user_info.s_nick_name;
                                    let content = &danmu_content.s_content;
                                    let color = danmu_content.t_bullet_format.i_color;

                                    if !content.is_empty() {
                                        let mut danmu = DanmuMessage::chat(
                                            push_msg.l_msg_id.to_string(),
                                            danmu_content.t_user_info.l_uid.to_string(),
                                            username.clone(),
                                            content.clone(),
                                        );
                                        if color != -1 && color != 0 {
                                            danmu = danmu.with_color(format!("#{:06X}", color));
                                        }
                                        return Ok(vec![danmu]);
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to convert to MessageNotice: {}", e);
                                }
                            }
                        }
                        Ok(vec![])
                    }
                    Err(e) => {
                        debug!("Failed to convert to WsPushMessage: {}", e);
                        Ok(vec![])
                    }
                }
            }
            // RegisterGroupRsp (17) - acknowledgment of group registration
            17 => {
                debug!("Successfully registered to group");
                Ok(vec![])
            }
            // Other types
            _ => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::danmaku::provider::DanmuProvider;

    /// Test that the protocol can extract room ID from various URL formats
    #[test]
    fn test_extract_room_id() {
        let protocol = HuyaDanmuProtocol::default();

        assert_eq!(
            protocol.extract_room_id("https://www.huya.com/123456"),
            Some("123456".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("http://huya.com/789012"),
            Some("789012".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("huya.com/111222"),
            Some("111222".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("https://www.huya.com/invalid"),
            None
        );
    }

    /// Test that living info packet can be created
    #[test]
    fn test_create_living_info_packet() {
        let result = HuyaDanmuProtocol::create_living_info_packet(
            294636272,
            "webh5&2512200618&websocket",
            "chrome",
        );
        assert!(result.is_ok());
        let packet = result.unwrap();
        std::fs::write("living_info_packet.bin", packet.clone()).unwrap();
        assert!(!packet.is_empty());
    }

    /// Test that do launch packet can be created
    #[test]
    fn test_create_do_launch_packet() {
        let result =
            HuyaDanmuProtocol::create_do_launch_packet("webh5&2512200618&websocket", "chrome");
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert!(!packet.is_empty());
    }

    /// Test that join group packet can be created
    #[test]
    fn test_create_join_group_packet() {
        let result = HuyaDanmuProtocol::create_create_join_group_packet(294636272);
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert!(!packet.is_empty());
    }

    /// Integration test: Connect to a real Huya room and verify handshake
    /// This test requires network access and a live Huya room
    #[tokio::test]
    #[ignore] // Run with: cargo test --package platforms -- --ignored test_real_connection
    async fn test_real_connection() {
        use std::time::Duration;
        use tokio::time::timeout;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();

        // Use a popular room that's likely to be live
        let room_id = "660000";

        let provider = HuyaDanmuProvider::new();

        // Test connection with timeout
        let connect_result = timeout(
            Duration::from_secs(10),
            provider.connect(room_id, ConnectionConfig::default()),
        )
        .await;

        assert!(connect_result.is_ok(), "Connection timed out");
        let mut connection = connect_result.unwrap().expect("Failed to connect");

        assert!(connection.is_connected, "Connection should be connected");

        // Try to receive messages for a few seconds
        let mut message_count = 0;
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(600) {
            match timeout(Duration::from_millis(500), async {
                provider.receive(&connection).await
            })
            .await
            {
                Ok(Ok(Some(msg))) => {
                    println!("{}: {}", msg.username, msg.content);
                    message_count += 1;
                }
                Ok(Ok(None)) => {
                    // No message, continue
                }
                Ok(Err(e)) => {
                    println!("Error receiving: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout, continue
                }
            }

            // Give the connection time to receive messages
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        println!("Received {} danmu messages", message_count);

        // Disconnect
        let disconnect_result = provider.disconnect(&mut connection).await;
        assert!(disconnect_result.is_ok(), "Failed to disconnect");
    }

    /// Integration test: Verify WebSocket command decoding
    #[tokio::test]
    #[ignore]
    async fn test_websocket_command_flow() {
        use futures::{SinkExt, StreamExt};
        use std::time::Duration;
        use tokio::time::timeout;
        use tokio_tungstenite::connect_async;

        let room_id = "660000";
        let protocol = HuyaDanmuProtocol::default();

        // Connect directly to verify the flow
        let ws_url = protocol.websocket_url(room_id).await.unwrap();
        println!("Connecting to: {}", ws_url);

        let (mut ws_stream, _) = timeout(Duration::from_secs(5), connect_async(&ws_url))
            .await
            .expect("Connection timed out")
            .expect("Failed to connect");

        // Send handshake messages
        let handshake_msgs = protocol.handshake_messages(room_id).await.unwrap();
        for msg in handshake_msgs {
            ws_stream.send(msg).await.expect("Failed to send handshake");
        }
        println!("Sent handshake messages");

        // Create a channel for responses
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);

        // Receive a few messages and verify WebSocketCommand decoding
        let mut response_count = 0;
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(10) && response_count < 20 {
            tokio::select! {
                Some(response_msg) = rx.recv() => {
                    // Send any response messages back
                    if let Err(e) = ws_stream.send(response_msg).await {
                        println!("Failed to send response: {}", e);
                    }
                }
                msg_result = ws_stream.next() => {
                    match msg_result {
                        Some(Ok(Message::Binary(data))) => {
                            // Try to decode as WebSocketCommand using manual pattern
                            if let Ok(tars_val) = tars_codec::decode_tars_value(data.clone()) {
                                if let Ok(cmd) = WebSocketCommand::try_from(tars_val) {
                                    let _cmd_type = cmd.cmd_type();
                                    response_count += 1;

                                    // Parse through our protocol
                                    match protocol.parse_socket_command(&data, room_id, &tx).await {
                                        Ok(danmus) => {
                                            for danmu in danmus {
                                                println!("  -> Danmu: {} - {}", danmu.username, danmu.content);
                                            }
                                        }
                                        Err(e) => {
                                            println!("  -> Parse error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(other)) => {
                            println!("Received other message: {:?}", other);
                        }
                        Some(Err(e)) => {
                            println!("WebSocket error: {}", e);
                            break;
                        }
                        None => {
                            println!("WebSocket closed");
                            break;
                        }
                    }
                }
            }
        }

        println!("Received {} WebSocket commands", response_count);
        assert!(
            response_count > 0,
            "Should have received at least one WebSocket command"
        );

        let _ = ws_stream.close(None).await;
    }
}
