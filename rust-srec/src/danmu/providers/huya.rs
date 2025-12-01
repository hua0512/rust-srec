//! Huya (虎牙) danmu provider.
//!
//! Implements danmu collection for the Huya streaming platform using WebSocket
//! with TARS protocol for message encoding/decoding.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use regex::Regex;
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tars_codec::{TarsMessage, TarsRequestHeader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};
use tracing::{debug, error, info, warn};

use crate::danmu::{DanmuConnection, DanmuMessage, DanmuProvider, DanmuType};
use crate::error::{Error, Result};

/// Huya WebSocket server URL
const HUYA_WS_URL: &str = "wss://cdnws.api.huya.com";

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Maximum reconnection attempts
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Base delay for exponential backoff (in milliseconds)
const BASE_RECONNECT_DELAY_MS: u64 = 1000;

/// Maximum delay for exponential backoff (in milliseconds)
const MAX_RECONNECT_DELAY_MS: u64 = 60000;

/// Huya message URI constants for different message types.
/// These are the iUri values used in Huya's TARS protocol.
pub mod huya_uri {
    /// Chat message (弹幕消息)
    pub const MESSAGE_NOTICE: i32 = 1400;
    /// Gift message (礼物消息)
    pub const SEND_ITEM_SUB_BROADCAST: i32 = 6501;
    /// Noble enter notification (贵族进场)
    pub const NOBLE_ENTER_NOTICE: i32 = 6502;
    /// VIP enter notification (VIP进场)
    pub const VIP_ENTER_BANNER: i32 = 6503;
    /// User enter notification (用户进场)
    pub const USER_ENTER_NOTICE: i32 = 6504;
}

/// Huya WebSocket command types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HuyaWsCmd {
    /// Heartbeat request (client to server)
    HeartbeatReq = 1,
    /// Heartbeat response (server to client)
    HeartbeatRsp = 2,
    /// Register request (client to server)
    RegisterReq = 3,
    /// Register response (server to client)
    RegisterRsp = 4,
    /// Message push request (server to client)
    MsgPushReq = 5,
    /// WUP response (server to client)
    WupRsp = 7,
}

/// Huya TARS protocol packet types for WebSocket communication.
/// These represent the different packet structures used in the Huya danmu protocol.
#[derive(Debug, Clone)]
pub enum HuyaTarsPacket {
    /// Authentication packet for establishing identity with the server.
    /// Contains user credentials or anonymous session info.
    Auth(HuyaAuthPacket),
    /// Room join packet for subscribing to a specific room's danmu stream.
    RoomJoin(HuyaRoomJoinPacket),
    /// Heartbeat packet to maintain the WebSocket connection.
    Heartbeat(HuyaHeartbeatPacket),
}

/// Authentication packet structure for Huya WebSocket connection.
/// Used to establish identity with the server (anonymous or authenticated).
#[derive(Debug, Clone)]
pub struct HuyaAuthPacket {
    /// User ID (0 for anonymous users)
    pub user_id: i64,
    /// Authentication token (empty string for anonymous)
    pub token: String,
    /// Unique session identifier (UUID)
    pub guid: String,
    /// Application ID (default: 0)
    pub app_id: i32,
}

impl Default for HuyaAuthPacket {
    fn default() -> Self {
        Self {
            user_id: 0,
            token: String::new(),
            guid: uuid::Uuid::new_v4().to_string(),
            app_id: 0,
        }
    }
}

/// Room join packet structure for subscribing to a room's danmu stream.
#[derive(Debug, Clone)]
pub struct HuyaRoomJoinPacket {
    /// The room ID to join
    pub room_id: i64,
    /// User ID (0 for anonymous)
    pub user_id: i64,
    /// Session token (empty for anonymous)
    pub token: String,
    /// Unique session identifier
    pub guid: String,
    /// Register type (1 = join room)
    pub register_type: i32,
}

impl HuyaRoomJoinPacket {
    /// Create a new room join packet for the specified room.
    pub fn new(room_id: i64) -> Self {
        Self {
            room_id,
            user_id: 0,
            token: String::new(),
            guid: uuid::Uuid::new_v4().to_string(),
            register_type: 1,
        }
    }
}

/// Heartbeat packet structure to maintain the WebSocket connection.
#[derive(Debug, Clone)]
pub struct HuyaHeartbeatPacket {
    /// Heartbeat cycle time in seconds
    pub cycle_time: i32,
}

impl Default for HuyaHeartbeatPacket {
    fn default() -> Self {
        Self { cycle_time: 0 }
    }
}

impl TryFrom<i32> for HuyaWsCmd {
    type Error = ();

    fn try_from(value: i32) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(HuyaWsCmd::HeartbeatReq),
            2 => Ok(HuyaWsCmd::HeartbeatRsp),
            3 => Ok(HuyaWsCmd::RegisterReq),
            4 => Ok(HuyaWsCmd::RegisterRsp),
            5 => Ok(HuyaWsCmd::MsgPushReq),
            7 => Ok(HuyaWsCmd::WupRsp),
            _ => Err(()),
        }
    }
}

/// Internal connection state for Huya WebSocket
struct HuyaConnectionState {
    /// WebSocket stream
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Room ID
    room_id: String,
    /// Whether the connection is active
    is_connected: Arc<AtomicBool>,
    /// Reconnection count
    reconnect_count: Arc<AtomicU32>,
    /// Message receiver channel
    message_rx: mpsc::Receiver<DanmuMessage>,
    /// Heartbeat task handle
    heartbeat_handle: Option<JoinHandle<()>>,
    /// Message processing task handle
    message_handle: Option<JoinHandle<()>>,
}

/// Shared state between connection and tasks
struct SharedConnectionState {
    /// Whether the connection is active
    is_connected: Arc<AtomicBool>,
    /// Reconnection count
    reconnect_count: Arc<AtomicU32>,
    /// Message sender channel
    message_tx: mpsc::Sender<DanmuMessage>,
}

/// Huya danmu provider.
pub struct HuyaDanmuProvider {
    /// Regex for extracting room ID from URL
    url_regex: OnceLock<Regex>,
    /// Active connections (connection_id -> state)
    connections: RwLock<FxHashMap<String, Arc<Mutex<HuyaConnectionState>>>>,
}

impl HuyaDanmuProvider {
    /// Create a new Huya danmu provider.
    pub fn new() -> Self {
        Self {
            url_regex: OnceLock::new(),
            connections: RwLock::new(FxHashMap::default()),
        }
    }

    fn get_url_regex(&self) -> &Regex {
        self.url_regex
            .get_or_init(|| Regex::new(r"(?:https?://)?(?:www\.)?huya\.com/(\d+)").unwrap())
    }

    /// Connect to Huya WebSocket server
    async fn connect_ws(
        &self,
        room_id: &str,
    ) -> Result<(
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        SharedConnectionState,
        mpsc::Receiver<DanmuMessage>,
    )> {
        let url = format!("{}?roomId={}", HUYA_WS_URL, room_id);
        info!("Connecting to Huya WebSocket: {}", url);

        let (ws_stream, _) = connect_async(&url).await.map_err(|e| {
            Error::DanmuError(format!("Failed to connect to Huya WebSocket: {}", e))
        })?;

        info!("Connected to Huya WebSocket for room {}", room_id);

        let (message_tx, message_rx) = mpsc::channel(1000);
        let is_connected = Arc::new(AtomicBool::new(true));
        let reconnect_count = Arc::new(AtomicU32::new(0));

        let shared_state = SharedConnectionState {
            is_connected,
            reconnect_count,
            message_tx,
        };

        Ok((ws_stream, shared_state, message_rx))
    }

    /// Create a TARS-encoded authentication packet for establishing identity.
    ///
    /// This packet is sent first to authenticate with the Huya server.
    /// For anonymous connections, use `HuyaAuthPacket::default()`.
    ///
    /// # Arguments
    /// * `auth` - The authentication packet containing user credentials or anonymous session info
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded authentication packet
    pub fn create_auth_packet(auth: &HuyaAuthPacket) -> Result<Bytes> {
        // Create the authentication request body
        let mut body_ser = tars_codec::ser::TarsSerializer::new();

        // Write lUid (tag 0) - user ID
        body_ser.write_i64(0, auth.user_id)?;
        // Write sToken (tag 1) - authentication token
        body_ser.write_string(1, &auth.token)?;
        // Write sGuid (tag 2) - unique session identifier
        body_ser.write_string(2, &auth.guid)?;
        // Write iAppId (tag 3) - application ID
        body_ser.write_i32(3, auth.app_id)?;

        let body_bytes = body_ser.into_bytes();

        // Create the TARS message
        let mut body_map = FxHashMap::default();
        body_map.insert("tReq".to_string(), body_bytes);

        let message = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: 0,
                servant_name: "huyalive".to_string(),
                func_name: "doLaunch".to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: body_map,
        };

        let encoded = tars_codec::encode_request(&message)
            .map_err(|e| Error::DanmuError(format!("Failed to encode auth packet: {}", e)))?;

        Ok(encoded.freeze())
    }

    /// Create a TARS-encoded room join packet for subscribing to a room's danmu stream.
    ///
    /// This packet is sent after authentication to join a specific room.
    ///
    /// # Arguments
    /// * `room_join` - The room join packet containing room ID and session info
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded room join packet
    pub fn create_room_join_packet(room_join: &HuyaRoomJoinPacket) -> Result<Bytes> {
        // Create the register request body
        let mut body_ser = tars_codec::ser::TarsSerializer::new();

        // Write tId (tag 0) - topic ID / room ID
        body_ser.write_i64(0, room_join.room_id)?;
        // Write sToken (tag 1) - session token
        body_ser.write_string(1, &room_join.token)?;
        // Write lUid (tag 2) - user ID
        body_ser.write_i64(2, room_join.user_id)?;
        // Write eType (tag 3) - register type (1 = join room)
        body_ser.write_i32(3, room_join.register_type)?;
        // Write sGuid (tag 4) - unique session identifier
        body_ser.write_string(4, &room_join.guid)?;

        let body_bytes = body_ser.into_bytes();

        // Create the TARS message
        let mut body_map = FxHashMap::default();
        body_map.insert("tReq".to_string(), body_bytes);

        let message = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: 0,
                servant_name: "huyalive".to_string(),
                func_name: "registerReq".to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: body_map,
        };

        let encoded = tars_codec::encode_request(&message)
            .map_err(|e| Error::DanmuError(format!("Failed to encode room join packet: {}", e)))?;

        Ok(encoded.freeze())
    }

    /// Create a TARS-encoded register packet for joining a room (legacy method).
    ///
    /// This is a convenience method that creates a room join packet from a room ID string.
    /// For more control, use `create_room_join_packet` with a `HuyaRoomJoinPacket`.
    ///
    /// # Arguments
    /// * `room_id` - The room ID as a string
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded register packet
    pub fn create_register_packet(room_id: &str) -> Result<Bytes> {
        let room_id_num: i64 = room_id
            .parse()
            .map_err(|_| Error::DanmuError("Invalid room ID".to_string()))?;

        let room_join = HuyaRoomJoinPacket::new(room_id_num);
        Self::create_room_join_packet(&room_join)
    }

    /// Create a TARS-encoded heartbeat packet to maintain the WebSocket connection.
    ///
    /// This packet should be sent at regular intervals (typically every 30 seconds)
    /// to keep the connection alive.
    ///
    /// # Arguments
    /// * `heartbeat` - The heartbeat packet configuration
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded heartbeat packet
    pub fn create_heartbeat_packet_with_config(heartbeat: &HuyaHeartbeatPacket) -> Result<Bytes> {
        // Create the heartbeat request body
        let mut body_ser = tars_codec::ser::TarsSerializer::new();

        // Write cycleTime (tag 0) - heartbeat cycle time
        body_ser.write_i32(0, heartbeat.cycle_time)?;

        let body_bytes = body_ser.into_bytes();

        // Create the TARS message
        let mut body_map = FxHashMap::default();
        body_map.insert("cycleTime".to_string(), body_bytes);

        let message = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: 0,
                servant_name: "huyalive".to_string(),
                func_name: "heartbeat".to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: body_map,
        };

        let encoded = tars_codec::encode_request(&message)
            .map_err(|e| Error::DanmuError(format!("Failed to encode heartbeat packet: {}", e)))?;

        Ok(encoded.freeze())
    }

    /// Create a TARS-encoded heartbeat packet with default configuration.
    ///
    /// This is a convenience method that creates a heartbeat packet with default settings.
    /// For more control, use `create_heartbeat_packet_with_config`.
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded heartbeat packet
    pub fn create_heartbeat_packet() -> Result<Bytes> {
        Self::create_heartbeat_packet_with_config(&HuyaHeartbeatPacket::default())
    }

    /// Create a TARS packet from a `HuyaTarsPacket` enum.
    ///
    /// This is a unified method for creating any type of Huya TARS packet.
    ///
    /// # Arguments
    /// * `packet` - The packet to encode
    ///
    /// # Returns
    /// * `Result<Bytes>` - The TARS-encoded packet
    pub fn create_tars_packet(packet: &HuyaTarsPacket) -> Result<Bytes> {
        match packet {
            HuyaTarsPacket::Auth(auth) => Self::create_auth_packet(auth),
            HuyaTarsPacket::RoomJoin(room_join) => Self::create_room_join_packet(room_join),
            HuyaTarsPacket::Heartbeat(heartbeat) => {
                Self::create_heartbeat_packet_with_config(heartbeat)
            }
        }
    }

    /// Parse a TARS message from raw WebSocket data.
    ///
    /// This function decodes the raw WebSocket binary data using the TARS protocol
    /// and extracts danmu messages based on the message type.
    ///
    /// # Arguments
    /// * `data` - Raw binary data from WebSocket
    ///
    /// # Returns
    /// * `Result<Option<DanmuMessage>>` - Parsed danmu message or None if not a chat message
    pub fn parse_tars_message(data: &[u8]) -> Result<Option<DanmuMessage>> {
        if data.len() < 4 {
            return Ok(None);
        }

        let mut src = BytesMut::from(data);
        let message = tars_codec::decode_response(&mut src)
            .map_err(|e| Error::DanmuError(format!("Failed to decode TARS message: {}", e)))?;

        let Some(message) = message else {
            return Ok(None);
        };

        // Check the function name to determine message type
        let func_name = &message.header.func_name;

        match func_name.as_str() {
            "registerRsp" => {
                debug!("Received register response");
                Ok(None)
            }
            "heartbeat" => {
                debug!("Received heartbeat response");
                Ok(None)
            }
            "pushMessage" | "onPushMessage" => {
                // Parse the push message body
                Self::parse_push_message(&message)
            }
            _ => {
                debug!("Received unknown message type: {}", func_name);
                Ok(None)
            }
        }
    }

    /// Parse a push message to extract danmu content.
    ///
    /// Huya push messages contain nested TARS structures with different URIs
    /// indicating the message type (chat, gift, etc.).
    fn parse_push_message(message: &TarsMessage) -> Result<Option<DanmuMessage>> {
        // Try to extract the message body from known keys
        for (key, body_bytes) in &message.body {
            if key == "vData" || key == "sMsg" || key.contains("Msg") || key == "tReq" {
                // Try to parse as a nested TARS structure
                if let Ok(Some(danmu)) = Self::parse_danmu_from_bytes(body_bytes) {
                    return Ok(Some(danmu));
                }
            }
        }

        // Try all body entries if specific keys didn't work
        for (_key, body_bytes) in &message.body {
            if let Ok(Some(danmu)) = Self::parse_danmu_from_bytes(body_bytes) {
                return Ok(Some(danmu));
            }
        }

        Ok(None)
    }

    /// Parse danmu message from TARS-encoded bytes.
    ///
    /// This function handles the nested TARS structure used by Huya for chat messages.
    /// The structure typically contains:
    /// - iUri (tag 0): Message type identifier
    /// - sMsg (tag 1): Nested message content as bytes
    ///
    /// For chat messages (iUri = 1400), the nested content contains:
    /// - tUserInfo (tag 0): User information struct
    /// - sContent (tag 2): Message content
    ///
    /// # Arguments
    /// * `data` - TARS-encoded bytes containing the message
    ///
    /// # Returns
    /// * `Result<Option<DanmuMessage>>` - Parsed danmu message or None
    fn parse_danmu_from_bytes(data: &Bytes) -> Result<Option<DanmuMessage>> {
        if data.is_empty() {
            return Ok(None);
        }

        // Try to decode as a TARS value - first try as a struct
        let value = match tars_codec::de::from_bytes(data.clone()) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        // Extract fields from the TARS value
        if let tars_codec::TarsValue::Struct(fields) = value {
            // Check if this is a Huya push message with iUri
            let uri = fields.get(&0).and_then(|v| v.as_i32());

            match uri {
                Some(huya_uri::MESSAGE_NOTICE) => {
                    // Chat message - parse the nested content
                    Self::parse_chat_message_content(&fields)
                }
                Some(huya_uri::SEND_ITEM_SUB_BROADCAST) => {
                    // Gift message
                    Self::parse_gift_message_content(&fields)
                }
                Some(huya_uri::USER_ENTER_NOTICE)
                | Some(huya_uri::NOBLE_ENTER_NOTICE)
                | Some(huya_uri::VIP_ENTER_BANNER) => {
                    // User enter notifications - skip these
                    Ok(None)
                }
                _ => {
                    // Try to parse as a direct chat message structure
                    Self::parse_direct_chat_message(&fields)
                }
            }
        } else {
            // If not a struct, try to parse as a sequence of tagged values
            Self::parse_tagged_values_from_bytes(data)
        }
    }

    /// Parse a sequence of tagged TARS values from bytes.
    ///
    /// This handles the case where the data contains multiple tagged values
    /// at the top level (not wrapped in a struct).
    fn parse_tagged_values_from_bytes(data: &Bytes) -> Result<Option<DanmuMessage>> {
        use tars_codec::de::TarsDeserializer;

        let mut deserializer = TarsDeserializer::new(data.clone());
        let mut fields = rustc_hash::FxHashMap::default();

        // Read all tagged values
        while !deserializer.is_empty() {
            match deserializer.read_value() {
                Ok((tag, value)) => {
                    fields.insert(tag, value);
                }
                Err(_) => break,
            }
        }

        if fields.is_empty() {
            return Ok(None);
        }

        // Check if this is a Huya push message with iUri
        let uri = fields.get(&0).and_then(|v| v.as_i32());

        match uri {
            Some(huya_uri::MESSAGE_NOTICE) => Self::parse_chat_message_content(&fields),
            Some(huya_uri::SEND_ITEM_SUB_BROADCAST) => Self::parse_gift_message_content(&fields),
            Some(huya_uri::USER_ENTER_NOTICE)
            | Some(huya_uri::NOBLE_ENTER_NOTICE)
            | Some(huya_uri::VIP_ENTER_BANNER) => Ok(None),
            _ => Self::parse_direct_chat_message(&fields),
        }
    }

    /// Parse chat message content from Huya's MessageNotice structure.
    ///
    /// The MessageNotice structure (iUri = 1400) contains:
    /// - tag 0: tUserInfo (user information struct)
    /// - tag 2: sContent (message content string)
    fn parse_chat_message_content(
        fields: &rustc_hash::FxHashMap<u8, tars_codec::TarsValue>,
    ) -> Result<Option<DanmuMessage>> {
        // Get the nested message bytes from tag 1 (sMsg)
        let msg_bytes = match fields.get(&1) {
            Some(tars_codec::TarsValue::SimpleList(bytes)) => bytes.clone(),
            Some(tars_codec::TarsValue::Binary(bytes)) => bytes.clone(),
            _ => {
                // Try to parse directly from the fields
                return Self::parse_direct_chat_message(fields);
            }
        };

        // Parse the nested message
        let nested_value = match tars_codec::de::from_bytes(msg_bytes) {
            Ok(v) => v,
            Err(_) => return Self::parse_direct_chat_message(fields),
        };

        if let tars_codec::TarsValue::Struct(nested_fields) = nested_value {
            let mut user_id = String::new();
            let mut username = String::new();
            let mut content = String::new();

            // Extract user info from tag 0 (tUserInfo)
            if let Some(tars_codec::TarsValue::Struct(user_info)) = nested_fields.get(&0) {
                // lUid - user ID (tag 0 in user info)
                if let Some(uid_val) = user_info.get(&0) {
                    // Handle both i32 and i64 user IDs
                    match uid_val {
                        tars_codec::TarsValue::Long(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Int(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Short(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Byte(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::String(uid_str) => {
                            user_id = uid_str.clone();
                        }
                        tars_codec::TarsValue::StringRef(uid_bytes) => {
                            if let Ok(uid_str) = std::str::from_utf8(uid_bytes) {
                                user_id = uid_str.to_string();
                            }
                        }
                        _ => {}
                    }
                }
                // sNickName - nickname (tag 1 in user info)
                if let Some(name_val) = user_info.get(&1) {
                    if let Some(name) = name_val.as_str() {
                        username = name.to_string();
                    }
                }
            }

            // Extract content from tag 2 (sContent)
            if let Some(content_val) = nested_fields.get(&2) {
                if let Some(text) = content_val.as_str() {
                    content = text.to_string();
                }
            }

            if !content.is_empty() {
                return Ok(Some(DanmuMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: if user_id.is_empty() {
                        "unknown".to_string()
                    } else {
                        user_id
                    },
                    username: if username.is_empty() {
                        "Unknown".to_string()
                    } else {
                        username
                    },
                    content,
                    timestamp: Utc::now(),
                    message_type: DanmuType::Chat,
                    metadata: None,
                }));
            }
        }

        Ok(None)
    }

    /// Parse gift message content from Huya's SendItemSubBroadcastPacket structure.
    fn parse_gift_message_content(
        fields: &rustc_hash::FxHashMap<u8, tars_codec::TarsValue>,
    ) -> Result<Option<DanmuMessage>> {
        // Get the nested message bytes from tag 1 (sMsg)
        let msg_bytes = match fields.get(&1) {
            Some(tars_codec::TarsValue::SimpleList(bytes)) => bytes.clone(),
            Some(tars_codec::TarsValue::Binary(bytes)) => bytes.clone(),
            _ => return Ok(None),
        };

        // Parse the nested message
        let nested_value = match tars_codec::de::from_bytes(msg_bytes) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        if let tars_codec::TarsValue::Struct(nested_fields) = nested_value {
            let mut user_id = String::new();
            let mut username = String::new();
            let mut gift_name = String::new();
            let mut gift_count: u32 = 1;

            // Extract sender info
            if let Some(tars_codec::TarsValue::Struct(sender_info)) = nested_fields.get(&0) {
                if let Some(uid_val) = sender_info.get(&0) {
                    // Handle both i32 and i64 user IDs
                    match uid_val {
                        tars_codec::TarsValue::Long(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Int(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Short(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Byte(uid) => {
                            user_id = uid.to_string();
                        }
                        _ => {}
                    }
                }
                if let Some(name_val) = sender_info.get(&1) {
                    if let Some(name) = name_val.as_str() {
                        username = name.to_string();
                    }
                }
            }

            // Extract gift info
            if let Some(gift_val) = nested_fields.get(&1) {
                if let Some(name) = gift_val.as_str() {
                    gift_name = name.to_string();
                }
            }
            if let Some(count_val) = nested_fields.get(&2) {
                if let Some(count) = count_val.as_i32() {
                    gift_count = count as u32;
                }
            }

            if !gift_name.is_empty() || !username.is_empty() {
                let mut danmu = DanmuMessage::gift(
                    uuid::Uuid::new_v4().to_string(),
                    if user_id.is_empty() {
                        "unknown".to_string()
                    } else {
                        user_id
                    },
                    if username.is_empty() {
                        "Unknown".to_string()
                    } else {
                        username
                    },
                    gift_name,
                    gift_count,
                );
                danmu.timestamp = Utc::now();
                return Ok(Some(danmu));
            }
        }

        Ok(None)
    }

    /// Parse a direct chat message structure (fallback for simpler formats).
    ///
    /// This handles cases where the message is not wrapped in the standard
    /// Huya push message format.
    fn parse_direct_chat_message(
        fields: &rustc_hash::FxHashMap<u8, tars_codec::TarsValue>,
    ) -> Result<Option<DanmuMessage>> {
        let mut user_id = String::new();
        let mut username = String::new();
        let mut content = String::new();
        let mut message_type = DanmuType::Chat;

        for (tag, field_value) in fields {
            match tag {
                // Common field tags for Huya danmu messages
                0 => {
                    // Could be lSenderUid or tUserInfo
                    // Handle both i32 and i64 user IDs
                    match field_value {
                        tars_codec::TarsValue::Long(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Int(uid) => {
                            user_id = uid.to_string();
                        }
                        tars_codec::TarsValue::Short(uid) => {
                            user_id = (*uid as i32).to_string();
                        }
                        tars_codec::TarsValue::Byte(uid) => {
                            user_id = (*uid as i32).to_string();
                        }
                        tars_codec::TarsValue::String(uid_str) => {
                            user_id = uid_str.clone();
                        }
                        tars_codec::TarsValue::StringRef(uid_bytes) => {
                            if let Ok(uid_str) = std::str::from_utf8(uid_bytes) {
                                user_id = uid_str.to_string();
                            }
                        }
                        tars_codec::TarsValue::Struct(user_info) => {
                            // Nested user info struct
                            if let Some(uid_val) = user_info.get(&0) {
                                match uid_val {
                                    tars_codec::TarsValue::Long(uid) => {
                                        user_id = uid.to_string();
                                    }
                                    tars_codec::TarsValue::Int(uid) => {
                                        user_id = uid.to_string();
                                    }
                                    tars_codec::TarsValue::Short(uid) => {
                                        user_id = (*uid as i32).to_string();
                                    }
                                    tars_codec::TarsValue::Byte(uid) => {
                                        user_id = (*uid as i32).to_string();
                                    }
                                    _ => {}
                                }
                            }
                            if let Some(name_val) = user_info.get(&1) {
                                if let Some(name) = name_val.as_str() {
                                    username = name.to_string();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                1 => {
                    // sNickName - sender nickname
                    if let Some(name) = field_value.as_str() {
                        username = name.to_string();
                    }
                }
                2 => {
                    // sContent - message content
                    if let Some(text) = field_value.as_str() {
                        content = text.to_string();
                    }
                }
                3 => {
                    // iMsgType - message type
                    if let Some(msg_type) = field_value.as_i32() {
                        message_type = match msg_type {
                            0 => DanmuType::Chat,
                            1 => DanmuType::Gift,
                            2 => DanmuType::SuperChat,
                            _ => DanmuType::Other,
                        };
                    }
                }
                _ => {}
            }
        }

        if !content.is_empty() || !username.is_empty() {
            return Ok(Some(DanmuMessage {
                id: uuid::Uuid::new_v4().to_string(),
                user_id: if user_id.is_empty() {
                    "unknown".to_string()
                } else {
                    user_id
                },
                username: if username.is_empty() {
                    "Unknown".to_string()
                } else {
                    username
                },
                content,
                timestamp: Utc::now(),
                message_type,
                metadata: None,
            }));
        }

        Ok(None)
    }

    /// Create a test chat message in Huya's TARS format.
    ///
    /// This is useful for testing the parsing logic without a live connection.
    /// The format matches what the parser expects:
    /// - Outer: tagged values with iUri (tag 0) and sMsg (tag 1 as SimpleList)
    /// - Inner (sMsg): struct with tUserInfo (tag 0 as Struct) and sContent (tag 2)
    /// - User info: struct with lUid (tag 0) and sNickName (tag 1)
    ///
    /// # Arguments
    /// * `user_id` - The user ID
    /// * `username` - The username/nickname
    /// * `content` - The message content
    ///
    /// # Returns
    /// * `Result<Bytes>` - TARS-encoded chat message
    #[cfg(test)]
    pub fn create_test_chat_message(user_id: i64, username: &str, content: &str) -> Result<Bytes> {
        use rustc_hash::FxHashMap;
        use tars_codec::TarsValue;
        use tars_codec::ser::TarsSerializer;

        // Create user info as a proper struct
        let mut user_fields: FxHashMap<u8, TarsValue> = FxHashMap::default();
        user_fields.insert(0, TarsValue::Long(user_id));
        user_fields.insert(1, TarsValue::String(username.to_string()));

        // Create the inner message struct
        let mut inner_fields: FxHashMap<u8, TarsValue> = FxHashMap::default();
        inner_fields.insert(0, TarsValue::Struct(user_fields));
        inner_fields.insert(2, TarsValue::String(content.to_string()));

        // Serialize the inner struct
        let mut inner_ser = TarsSerializer::new();
        inner_ser.write_struct(0, &inner_fields)?;
        let inner_bytes = inner_ser.into_bytes();

        // Create the outer push message with iUri and sMsg
        let mut outer_ser = TarsSerializer::new();
        outer_ser.write_i32(0, huya_uri::MESSAGE_NOTICE)?; // iUri
        outer_ser.write_simple_list(1, &inner_bytes)?; // sMsg as SimpleList

        Ok(outer_ser.into_bytes())
    }

    /// Create a test gift message in Huya's TARS format.
    ///
    /// The format matches what the parser expects:
    /// - Outer: tagged values with iUri (tag 0) and sMsg (tag 1 as SimpleList)
    /// - Inner (sMsg): struct with sender info (tag 0 as Struct), gift name (tag 1), count (tag 2)
    /// - Sender info: struct with lUid (tag 0) and sNickName (tag 1)
    #[cfg(test)]
    pub fn create_test_gift_message(
        user_id: i64,
        username: &str,
        gift_name: &str,
        gift_count: i32,
    ) -> Result<Bytes> {
        use rustc_hash::FxHashMap;
        use tars_codec::TarsValue;
        use tars_codec::ser::TarsSerializer;

        // Create sender info as a proper struct
        let mut sender_fields: FxHashMap<u8, TarsValue> = FxHashMap::default();
        sender_fields.insert(0, TarsValue::Long(user_id));
        sender_fields.insert(1, TarsValue::String(username.to_string()));

        // Create the inner message struct
        let mut inner_fields: FxHashMap<u8, TarsValue> = FxHashMap::default();
        inner_fields.insert(0, TarsValue::Struct(sender_fields));
        inner_fields.insert(1, TarsValue::String(gift_name.to_string()));
        inner_fields.insert(2, TarsValue::Int(gift_count));

        // Serialize the inner struct
        let mut inner_ser = TarsSerializer::new();
        inner_ser.write_struct(0, &inner_fields)?;
        let inner_bytes = inner_ser.into_bytes();

        // Create the outer push message with iUri and sMsg
        let mut outer_ser = TarsSerializer::new();
        outer_ser.write_i32(0, huya_uri::SEND_ITEM_SUB_BROADCAST)?; // iUri
        outer_ser.write_simple_list(1, &inner_bytes)?; // sMsg as SimpleList

        Ok(outer_ser.into_bytes())
    }

    /// Start the heartbeat task
    fn start_heartbeat_task(
        ws_sender: Arc<
            Mutex<futures::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
        >,
        is_connected: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

            while is_connected.load(Ordering::SeqCst) {
                interval.tick().await;

                if !is_connected.load(Ordering::SeqCst) {
                    break;
                }

                match Self::create_heartbeat_packet() {
                    Ok(packet) => {
                        let mut sender = ws_sender.lock().await;
                        if let Err(e) = sender.send(Message::Binary(packet.to_vec().into())).await {
                            warn!("Failed to send heartbeat: {}", e);
                            is_connected.store(false, Ordering::SeqCst);
                            break;
                        }
                        debug!("Sent heartbeat packet");
                    }
                    Err(e) => {
                        error!("Failed to create heartbeat packet: {}", e);
                    }
                }
            }

            debug!("Heartbeat task stopped");
        })
    }

    /// Start the message processing task
    fn start_message_task(
        mut ws_receiver: futures::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        message_tx: mpsc::Sender<DanmuMessage>,
        is_connected: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while is_connected.load(Ordering::SeqCst) {
                match ws_receiver.next().await {
                    Some(Ok(Message::Binary(data))) => {
                        match Self::parse_tars_message(&data) {
                            Ok(Some(danmu)) => {
                                if message_tx.send(danmu).await.is_err() {
                                    debug!("Message channel closed");
                                    break;
                                }
                            }
                            Ok(None) => {
                                // Non-danmu message (heartbeat, register response, etc.)
                            }
                            Err(e) => {
                                debug!("Failed to parse message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("WebSocket connection closed by server");
                        is_connected.store(false, Ordering::SeqCst);
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        debug!("Received ping, will respond with pong");
                        // Pong is handled automatically by tungstenite
                        let _ = data;
                    }
                    Some(Ok(_)) => {
                        // Ignore other message types
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        is_connected.store(false, Ordering::SeqCst);
                        break;
                    }
                    None => {
                        info!("WebSocket stream ended");
                        is_connected.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }

            debug!("Message processing task stopped");
        })
    }

    /// Calculate reconnection delay with exponential backoff
    fn calculate_reconnect_delay(attempt: u32) -> Duration {
        let delay_ms = BASE_RECONNECT_DELAY_MS * 2u64.pow(attempt.min(10));
        Duration::from_millis(delay_ms.min(MAX_RECONNECT_DELAY_MS))
    }

    /// Attempt to reconnect with exponential backoff
    async fn reconnect(&self, connection_id: &str, room_id: &str) -> Result<()> {
        let mut attempt = 0;

        while attempt < MAX_RECONNECT_ATTEMPTS {
            let delay = Self::calculate_reconnect_delay(attempt);
            info!(
                "Attempting to reconnect to room {} (attempt {}/{}), waiting {:?}",
                room_id,
                attempt + 1,
                MAX_RECONNECT_ATTEMPTS,
                delay
            );

            tokio::time::sleep(delay).await;

            match self.connect_ws(room_id).await {
                Ok((ws_stream, shared_state, message_rx)) => {
                    // Send register packet
                    let (mut write, read) = ws_stream.split();
                    let register_packet = Self::create_register_packet(room_id)?;
                    write
                        .send(Message::Binary(register_packet.to_vec().into()))
                        .await
                        .map_err(|e| {
                            Error::DanmuError(format!("Failed to send register packet: {}", e))
                        })?;

                    let ws_sender = Arc::new(Mutex::new(write));

                    // Start background tasks
                    let heartbeat_handle = Self::start_heartbeat_task(
                        ws_sender.clone(),
                        shared_state.is_connected.clone(),
                    );

                    let message_handle = Self::start_message_task(
                        read,
                        shared_state.message_tx.clone(),
                        shared_state.is_connected.clone(),
                    );

                    // Update connection state
                    let mut connections = self.connections.write().await;
                    if let Some(conn_state) = connections.get_mut(connection_id) {
                        let mut state = conn_state.lock().await;
                        state.is_connected = shared_state.is_connected;
                        state.reconnect_count = shared_state.reconnect_count;
                        state.message_rx = message_rx;
                        state.heartbeat_handle = Some(heartbeat_handle);
                        state.message_handle = Some(message_handle);
                        state.reconnect_count.fetch_add(1, Ordering::SeqCst);
                    }

                    info!("Successfully reconnected to room {}", room_id);
                    return Ok(());
                }
                Err(e) => {
                    warn!("Reconnection attempt {} failed: {}", attempt + 1, e);
                    attempt += 1;
                }
            }
        }

        Err(Error::DanmuError(format!(
            "Failed to reconnect after {} attempts",
            MAX_RECONNECT_ATTEMPTS
        )))
    }
}

impl Default for HuyaDanmuProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DanmuProvider for HuyaDanmuProvider {
    fn platform(&self) -> &str {
        "huya"
    }

    async fn connect(&self, room_id: &str) -> Result<DanmuConnection> {
        let connection_id = format!("huya-{}-{}", room_id, uuid::Uuid::new_v4());

        // Connect to WebSocket
        let (ws_stream, shared_state, message_rx) = self.connect_ws(room_id).await?;

        // Split the WebSocket stream
        let (mut write, read) = ws_stream.split();

        // Send register packet to join the room
        let register_packet = Self::create_register_packet(room_id)?;
        write
            .send(Message::Binary(register_packet.to_vec().into()))
            .await
            .map_err(|e| Error::DanmuError(format!("Failed to send register packet: {}", e)))?;

        info!("Sent register packet for room {}", room_id);

        let ws_sender = Arc::new(Mutex::new(write));

        // Start heartbeat task
        let heartbeat_handle =
            Self::start_heartbeat_task(ws_sender.clone(), shared_state.is_connected.clone());

        // Start message processing task
        let message_handle = Self::start_message_task(
            read,
            shared_state.message_tx.clone(),
            shared_state.is_connected.clone(),
        );

        // Create connection state
        let conn_state = HuyaConnectionState {
            ws_stream: tokio_tungstenite::WebSocketStream::from_raw_socket(
                // We can't reconstruct the stream, so we'll use a dummy approach
                // The actual stream is managed by the split sender/receiver
                unsafe { std::mem::zeroed() },
                tokio_tungstenite::tungstenite::protocol::Role::Client,
                None,
            )
            .await,
            room_id: room_id.to_string(),
            is_connected: shared_state.is_connected.clone(),
            reconnect_count: shared_state.reconnect_count.clone(),
            message_rx,
            heartbeat_handle: Some(heartbeat_handle),
            message_handle: Some(message_handle),
        };

        // Store connection state
        {
            let mut connections = self.connections.write().await;
            connections.insert(connection_id.clone(), Arc::new(Mutex::new(conn_state)));
        }

        // Create and return the connection handle
        let mut connection = DanmuConnection::new(connection_id, "huya", room_id);
        connection.set_connected();

        Ok(connection)
    }

    async fn disconnect(&self, connection: &mut DanmuConnection) -> Result<()> {
        let mut connections = self.connections.write().await;

        if let Some(conn_state) = connections.remove(&connection.id) {
            let mut state = conn_state.lock().await;

            // Stop the connection
            state.is_connected.store(false, Ordering::SeqCst);

            // Abort background tasks
            if let Some(handle) = state.heartbeat_handle.take() {
                handle.abort();
            }
            if let Some(handle) = state.message_handle.take() {
                handle.abort();
            }

            info!("Disconnected from Huya room {}", state.room_id);
        }

        connection.set_disconnected();
        Ok(())
    }

    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuMessage>> {
        if !connection.is_connected {
            return Err(Error::DanmuError("Connection is not active".to_string()));
        }

        let connections = self.connections.read().await;
        let conn_state = connections
            .get(&connection.id)
            .ok_or_else(|| Error::DanmuError(format!("Connection {} not found", connection.id)))?;

        let mut state = conn_state.lock().await;

        // Check if still connected
        if !state.is_connected.load(Ordering::SeqCst) {
            // Connection dropped, attempt reconnection
            drop(state);
            drop(connections);

            let room_id = connection.room_id.clone();
            self.reconnect(&connection.id, &room_id).await?;

            // Retry receiving after reconnection
            let connections = self.connections.read().await;
            if let Some(conn_state) = connections.get(&connection.id) {
                let mut state = conn_state.lock().await;
                return Ok(state.message_rx.recv().await);
            }

            return Err(Error::DanmuError("Failed to reconnect".to_string()));
        }

        // Try to receive a message with timeout
        match tokio::time::timeout(Duration::from_secs(1), state.message_rx.recv()).await {
            Ok(Some(msg)) => Ok(Some(msg)),
            Ok(None) => {
                // Channel closed, connection might be dead
                Ok(None)
            }
            Err(_) => {
                // Timeout, no message available
                Ok(None)
            }
        }
    }

    fn supports_url(&self, url: &str) -> bool {
        self.get_url_regex().is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        self.get_url_regex()
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url() {
        let provider = HuyaDanmuProvider::new();

        assert!(provider.supports_url("https://www.huya.com/12345"));
        assert!(provider.supports_url("http://huya.com/67890"));
        assert!(provider.supports_url("huya.com/11111"));

        assert!(!provider.supports_url("https://www.twitch.tv/streamer"));
        assert!(!provider.supports_url("https://www.douyu.com/12345"));
    }

    #[test]
    fn test_extract_room_id() {
        let provider = HuyaDanmuProvider::new();

        assert_eq!(
            provider.extract_room_id("https://www.huya.com/12345"),
            Some("12345".to_string())
        );
        assert_eq!(
            provider.extract_room_id("http://huya.com/67890"),
            Some("67890".to_string())
        );
        assert_eq!(
            provider.extract_room_id("https://www.twitch.tv/streamer"),
            None
        );
    }

    #[test]
    fn test_create_register_packet() {
        let packet = HuyaDanmuProvider::create_register_packet("12345");
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());
    }

    #[test]
    fn test_create_heartbeat_packet() {
        let packet = HuyaDanmuProvider::create_heartbeat_packet();
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());
    }

    #[test]
    fn test_create_auth_packet() {
        // Test with default (anonymous) auth
        let auth = HuyaAuthPacket::default();
        let packet = HuyaDanmuProvider::create_auth_packet(&auth);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());

        // Test with custom auth
        let custom_auth = HuyaAuthPacket {
            user_id: 12345,
            token: "test_token".to_string(),
            guid: "custom-guid-123".to_string(),
            app_id: 100,
        };
        let packet = HuyaDanmuProvider::create_auth_packet(&custom_auth);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());
    }

    #[test]
    fn test_create_room_join_packet() {
        // Test with default room join
        let room_join = HuyaRoomJoinPacket::new(12345);
        let packet = HuyaDanmuProvider::create_room_join_packet(&room_join);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());

        // Test with custom room join
        let custom_room_join = HuyaRoomJoinPacket {
            room_id: 67890,
            user_id: 111,
            token: "session_token".to_string(),
            guid: "custom-guid-456".to_string(),
            register_type: 1,
        };
        let packet = HuyaDanmuProvider::create_room_join_packet(&custom_room_join);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());
    }

    #[test]
    fn test_create_heartbeat_packet_with_config() {
        // Test with default heartbeat
        let heartbeat = HuyaHeartbeatPacket::default();
        let packet = HuyaDanmuProvider::create_heartbeat_packet_with_config(&heartbeat);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());

        // Test with custom cycle time
        let custom_heartbeat = HuyaHeartbeatPacket { cycle_time: 60 };
        let packet = HuyaDanmuProvider::create_heartbeat_packet_with_config(&custom_heartbeat);
        assert!(packet.is_ok());
        let packet = packet.unwrap();
        assert!(!packet.is_empty());
    }

    #[test]
    fn test_create_tars_packet() {
        // Test auth packet via unified method
        let auth_packet = HuyaTarsPacket::Auth(HuyaAuthPacket::default());
        let packet = HuyaDanmuProvider::create_tars_packet(&auth_packet);
        assert!(packet.is_ok());

        // Test room join packet via unified method
        let room_join_packet = HuyaTarsPacket::RoomJoin(HuyaRoomJoinPacket::new(12345));
        let packet = HuyaDanmuProvider::create_tars_packet(&room_join_packet);
        assert!(packet.is_ok());

        // Test heartbeat packet via unified method
        let heartbeat_packet = HuyaTarsPacket::Heartbeat(HuyaHeartbeatPacket::default());
        let packet = HuyaDanmuProvider::create_tars_packet(&heartbeat_packet);
        assert!(packet.is_ok());
    }

    #[test]
    fn test_register_packet_invalid_room_id() {
        // Test with invalid room ID (non-numeric)
        let packet = HuyaDanmuProvider::create_register_packet("invalid");
        assert!(packet.is_err());
    }

    #[test]
    fn test_calculate_reconnect_delay() {
        let delay0 = HuyaDanmuProvider::calculate_reconnect_delay(0);
        assert_eq!(delay0, Duration::from_millis(BASE_RECONNECT_DELAY_MS));

        let delay1 = HuyaDanmuProvider::calculate_reconnect_delay(1);
        assert_eq!(delay1, Duration::from_millis(BASE_RECONNECT_DELAY_MS * 2));

        let delay2 = HuyaDanmuProvider::calculate_reconnect_delay(2);
        assert_eq!(delay2, Duration::from_millis(BASE_RECONNECT_DELAY_MS * 4));

        // Test max delay cap
        let delay_max = HuyaDanmuProvider::calculate_reconnect_delay(20);
        assert_eq!(delay_max, Duration::from_millis(MAX_RECONNECT_DELAY_MS));
    }

    #[test]
    fn test_huya_ws_cmd_try_from() {
        assert_eq!(HuyaWsCmd::try_from(1), Ok(HuyaWsCmd::HeartbeatReq));
        assert_eq!(HuyaWsCmd::try_from(2), Ok(HuyaWsCmd::HeartbeatRsp));
        assert_eq!(HuyaWsCmd::try_from(3), Ok(HuyaWsCmd::RegisterReq));
        assert_eq!(HuyaWsCmd::try_from(4), Ok(HuyaWsCmd::RegisterRsp));
        assert_eq!(HuyaWsCmd::try_from(5), Ok(HuyaWsCmd::MsgPushReq));
        assert_eq!(HuyaWsCmd::try_from(7), Ok(HuyaWsCmd::WupRsp));
        assert_eq!(HuyaWsCmd::try_from(99), Err(()));
    }

    #[test]
    fn test_parse_tars_message_empty_data() {
        // Empty data should return None
        let result = HuyaDanmuProvider::parse_tars_message(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Data too short should return None
        let result = HuyaDanmuProvider::parse_tars_message(&[0, 1, 2]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_danmu_from_bytes_empty() {
        // Empty bytes should return None
        let result = HuyaDanmuProvider::parse_danmu_from_bytes(&Bytes::new());
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_direct_chat_message() {
        use tars_codec::ser::TarsSerializer;

        // Create a simple chat message structure
        let mut ser = TarsSerializer::new();
        ser.write_i64(0, 12345).unwrap(); // user_id
        ser.write_string(1, "TestUser").unwrap(); // username
        ser.write_string(2, "Hello, World!").unwrap(); // content
        ser.write_i32(3, 0).unwrap(); // message_type (Chat)
        let bytes = ser.into_bytes();

        let result = HuyaDanmuProvider::parse_danmu_from_bytes(&bytes);
        assert!(result.is_ok());
        let danmu = result.unwrap();
        assert!(danmu.is_some());

        let danmu = danmu.unwrap();
        assert_eq!(danmu.user_id, "12345");
        assert_eq!(danmu.username, "TestUser");
        assert_eq!(danmu.content, "Hello, World!");
        assert_eq!(danmu.message_type, DanmuType::Chat);
    }

    #[test]
    fn test_parse_direct_chat_message_gift_type() {
        use tars_codec::ser::TarsSerializer;

        // Create a gift message structure
        let mut ser = TarsSerializer::new();
        ser.write_i64(0, 67890).unwrap(); // user_id
        ser.write_string(1, "GiftUser").unwrap(); // username
        ser.write_string(2, "Sent a gift").unwrap(); // content
        ser.write_i32(3, 1).unwrap(); // message_type (Gift)
        let bytes = ser.into_bytes();

        let result = HuyaDanmuProvider::parse_danmu_from_bytes(&bytes);
        assert!(result.is_ok());
        let danmu = result.unwrap();
        assert!(danmu.is_some());

        let danmu = danmu.unwrap();
        assert_eq!(danmu.user_id, "67890");
        assert_eq!(danmu.username, "GiftUser");
        assert_eq!(danmu.message_type, DanmuType::Gift);
    }

    #[test]
    fn test_parse_direct_chat_message_with_missing_fields() {
        use tars_codec::ser::TarsSerializer;

        // Create a message with only content (missing user info)
        let mut ser = TarsSerializer::new();
        ser.write_string(2, "Just content").unwrap(); // content only
        let bytes = ser.into_bytes();

        let result = HuyaDanmuProvider::parse_danmu_from_bytes(&bytes);
        assert!(result.is_ok());
        let danmu = result.unwrap();
        assert!(danmu.is_some());

        let danmu = danmu.unwrap();
        assert_eq!(danmu.user_id, "unknown");
        assert_eq!(danmu.username, "Unknown");
        assert_eq!(danmu.content, "Just content");
    }

    #[test]
    fn test_huya_uri_constants() {
        // Verify URI constants are correct
        assert_eq!(huya_uri::MESSAGE_NOTICE, 1400);
        assert_eq!(huya_uri::SEND_ITEM_SUB_BROADCAST, 6501);
        assert_eq!(huya_uri::NOBLE_ENTER_NOTICE, 6502);
        assert_eq!(huya_uri::VIP_ENTER_BANNER, 6503);
        assert_eq!(huya_uri::USER_ENTER_NOTICE, 6504);
    }

    #[test]
    fn test_create_test_chat_message() {
        // Test the test helper function
        let result =
            HuyaDanmuProvider::create_test_chat_message(12345, "TestUser", "Hello from test!");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_create_test_gift_message() {
        // Test the gift message helper function
        let result = HuyaDanmuProvider::create_test_gift_message(67890, "GiftUser", "Rocket", 5);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // **Feature: jwt-auth-and-api-implementation, Property 9: TARS Message Parsing**
    // **Validates: Requirements 4.3, 4.6**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// Property: For any valid TARS-encoded Huya chat message, parsing it should
        /// produce a DanmuMessage with non-empty user ID, username, and content.
        #[test]
        fn prop_tars_chat_message_parsing(
            user_id in 1i64..=i64::MAX,
            username in "[a-zA-Z0-9_\u{4e00}-\u{9fff}]{1,30}",
            content in "[a-zA-Z0-9_\u{4e00}-\u{9fff} !?.,]{1,200}",
        ) {
            // Create a TARS-encoded chat message using the test helper
            let tars_bytes = HuyaDanmuProvider::create_test_chat_message(
                user_id,
                &username,
                &content,
            ).expect("Creating test chat message should succeed");

            // Parse the TARS message
            let result = HuyaDanmuProvider::parse_danmu_from_bytes(&tars_bytes);
            prop_assert!(result.is_ok(), "Parsing should not return an error");

            let danmu_opt = result.unwrap();
            prop_assert!(danmu_opt.is_some(), "Parsing should produce a DanmuMessage");

            let danmu = danmu_opt.unwrap();

            // Property: user_id should be non-empty and match the input
            prop_assert!(!danmu.user_id.is_empty(), "user_id should not be empty");
            prop_assert_eq!(
                danmu.user_id,
                user_id.to_string(),
                "user_id should match the input"
            );

            // Property: username should be non-empty and match the input
            prop_assert!(!danmu.username.is_empty(), "username should not be empty");
            prop_assert_eq!(
                danmu.username,
                username,
                "username should match the input"
            );

            // Property: content should be non-empty and match the input
            prop_assert!(!danmu.content.is_empty(), "content should not be empty");
            prop_assert_eq!(
                danmu.content,
                content,
                "content should match the input"
            );

            // Property: message_type should be Chat for chat messages
            prop_assert_eq!(
                danmu.message_type,
                DanmuType::Chat,
                "message_type should be Chat"
            );
        }

        /// Property: For any valid TARS-encoded Huya gift message, parsing it should
        /// produce a DanmuMessage with Gift type and non-empty user info.
        #[test]
        fn prop_tars_gift_message_parsing(
            user_id in 1i64..=i64::MAX,
            username in "[a-zA-Z0-9_\u{4e00}-\u{9fff}]{1,30}",
            gift_name in "[a-zA-Z0-9_\u{4e00}-\u{9fff}]{1,50}",
            gift_count in 1i32..=10000i32,
        ) {
            // Create a TARS-encoded gift message using the test helper
            let tars_bytes = HuyaDanmuProvider::create_test_gift_message(
                user_id,
                &username,
                &gift_name,
                gift_count,
            ).expect("Creating test gift message should succeed");

            // Parse the TARS message
            let result = HuyaDanmuProvider::parse_danmu_from_bytes(&tars_bytes);
            prop_assert!(result.is_ok(), "Parsing should not return an error");

            let danmu_opt = result.unwrap();
            prop_assert!(danmu_opt.is_some(), "Parsing should produce a DanmuMessage");

            let danmu = danmu_opt.unwrap();

            // Property: user_id should be non-empty and match the input
            prop_assert!(!danmu.user_id.is_empty(), "user_id should not be empty");
            prop_assert_eq!(
                danmu.user_id,
                user_id.to_string(),
                "user_id should match the input"
            );

            // Property: username should be non-empty and match the input
            prop_assert!(!danmu.username.is_empty(), "username should not be empty");
            prop_assert_eq!(
                danmu.username,
                username,
                "username should match the input"
            );

            // Property: message_type should be Gift for gift messages
            prop_assert_eq!(
                danmu.message_type,
                DanmuType::Gift,
                "message_type should be Gift"
            );
        }
    }
}
