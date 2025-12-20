//! Douyu danmu message models.
//!
//! Defines message types and structures used in the Douyu danmu protocol.

use rustc_hash::FxHashMap;

use super::stt::{stt_decode, stt_encode};

/// Message types used in the Douyu danmu protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DouyuMessageType {
    /// Login request message
    LoginReq,
    /// Login response message
    LoginRes,
    /// Join group request message
    JoinGroup,
    /// Heartbeat message (client -> server)
    Heartbeat,
    /// Keepalive message (server -> client)
    KeepAlive,
    /// Chat message
    ChatMsg,
    /// Gift message
    Gift,
    /// User enter room
    UserEnter,
    /// Unknown message type
    Unknown,
}

impl DouyuMessageType {
    /// Parse message type from STT type field.
    pub fn from_str(s: &str) -> Self {
        match s {
            "loginreq" => Self::LoginReq,
            "loginres" => Self::LoginRes,
            "joingroup" => Self::JoinGroup,
            "mrkl" => Self::Heartbeat,
            "keeplive" | "pingreq" => Self::KeepAlive,
            "chatmsg" => Self::ChatMsg,
            "dgb" => Self::Gift,
            "uenter" => Self::UserEnter,
            _ => Self::Unknown,
        }
    }

    /// Get the string representation for encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoginReq => "loginreq",
            Self::LoginRes => "loginres",
            Self::JoinGroup => "joingroup",
            Self::Heartbeat => "mrkl",
            Self::KeepAlive => "keeplive",
            Self::ChatMsg => "chatmsg",
            Self::Gift => "dgb",
            Self::UserEnter => "uenter",
            Self::Unknown => "unknown",
        }
    }
}

/// Create a login request message.
///
/// # Arguments
/// * `room_id` - The room ID to join
///
/// # Returns
/// STT-encoded login request string
pub fn create_login_message(room_id: &str) -> String {
    let mut map = FxHashMap::default();
    map.insert("type", "loginreq");
    map.insert("roomid", room_id);
    stt_encode(&map)
}

/// Create a join group request message.
///
/// # Arguments
/// * `room_id` - The room ID
/// * `gid` - Group ID (usually -9999 for main group)
///
/// # Returns
/// STT-encoded join group request string
pub fn create_join_group_message(room_id: &str, gid: i32) -> String {
    let gid_str = gid.to_string();
    let mut map = FxHashMap::default();
    map.insert("type", "joingroup");
    map.insert("rid", room_id);
    map.insert("gid", &gid_str);

    stt_encode(&map)
}

/// Parsed chat message from Douyu.
#[derive(Debug, Clone)]
pub struct DouyuChatMessage {
    /// Message ID
    pub id: String,
    /// Sender's nickname
    pub nickname: String,
    /// Sender's user ID
    pub uid: String,
    /// Chat content
    pub content: String,
    /// User level
    pub level: u32,
    /// Room ID
    pub room_id: String,
    /// Badge name (fan card name)
    pub badge_name: Option<String>,
    /// Badge level
    pub badge_level: Option<u32>,
    /// Platform (e.g., "pc_web", "android", "ios")
    pub platform: Option<String>,
    /// Noble level (贵族等级)
    pub noble_level: Option<u32>,
    /// Color (hex string)
    pub color: Option<String>,
}

impl DouyuChatMessage {
    /// Parse a chat message from decoded STT map.
    pub fn from_map(map: &FxHashMap<String, String>) -> Option<Self> {
        // Required fields
        let nickname = map.get("nn")?.clone();
        let content = map.get("txt")?.clone();

        // Optional fields with defaults
        let id = map.get("cid").cloned().unwrap_or_default();
        let uid = map.get("uid").cloned().unwrap_or_default();
        let room_id = map.get("rid").cloned().unwrap_or_default();
        let level = map.get("level").and_then(|s| s.parse().ok()).unwrap_or(0);

        let badge_name = map.get("bnn").cloned().filter(|s| !s.is_empty());
        let badge_level = map.get("bl").and_then(|s| s.parse().ok());
        let platform = map.get("plat").cloned().filter(|s| !s.is_empty());
        let noble_level = map.get("nl").and_then(|s| s.parse().ok());

        // Color is provided as an integer in the col field
        let color = map.get("col").and_then(|s| {
            s.parse::<u32>().ok().map(|c| {
                // Convert integer color to hex string
                // Douyu uses specific color codes like 1=red, 2=blue, etc.
                // or raw RGB values
                match c {
                    0 => "#FFFFFF".to_string(),               // White (default)
                    1 => "#FF0000".to_string(),               // Red
                    2 => "#1E90FF".to_string(),               // Blue
                    3 => "#00FF00".to_string(),               // Green
                    4 => "#FF7F00".to_string(),               // Orange
                    5 => "#FF00FF".to_string(),               // Purple
                    6 => "#00FFFF".to_string(),               // Cyan
                    _ if c > 0xFFFF => format!("#{:06X}", c), // RGB value
                    _ => "#FFFFFF".to_string(),
                }
            })
        });

        Some(Self {
            id,
            nickname,
            uid,
            content,
            level,
            room_id,
            badge_name,
            badge_level,
            platform,
            noble_level,
            color,
        })
    }
}

/// Parsed gift message from Douyu.
#[derive(Debug, Clone)]
pub struct DouyuGiftMessage {
    /// Sender's nickname
    pub nickname: String,
    /// Sender's user ID
    pub uid: String,
    /// Gift ID
    pub gift_id: String,
    /// Gift name
    pub gift_name: String,
    /// Gift count
    pub gift_count: u32,
    /// Room ID
    pub room_id: String,
    /// Gift hits/combo
    pub hits: u32,
}

impl DouyuGiftMessage {
    /// Parse a gift message from decoded STT map.
    pub fn from_map(map: &FxHashMap<String, String>) -> Option<Self> {
        // Required fields
        let nickname = map.get("nn")?.clone();
        let gift_id = map.get("gfid")?.clone();

        // Optional fields with defaults
        let uid = map.get("uid").cloned().unwrap_or_default();
        let room_id = map.get("rid").cloned().unwrap_or_default();
        let gift_name = map
            .get("gfname")
            .cloned()
            .unwrap_or_else(|| "Gift".to_string());
        let gift_count = map.get("gfcnt").and_then(|s| s.parse().ok()).unwrap_or(1);
        let hits = map.get("hits").and_then(|s| s.parse().ok()).unwrap_or(0);

        Some(Self {
            nickname,
            uid,
            gift_id,
            gift_name,
            gift_count,
            room_id,
            hits,
        })
    }
}

/// Parse the message type from an STT-decoded map.
pub fn get_message_type(map: &FxHashMap<String, String>) -> DouyuMessageType {
    map.get("type")
        .map(|s| DouyuMessageType::from_str(s))
        .unwrap_or(DouyuMessageType::Unknown)
}

/// Parse a raw STT payload and return the message type and decoded map.
pub fn parse_message(payload: &str) -> (DouyuMessageType, FxHashMap<String, String>) {
    let map = stt_decode(payload);
    let msg_type = get_message_type(&map);
    (msg_type, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_from_str() {
        assert_eq!(
            DouyuMessageType::from_str("loginreq"),
            DouyuMessageType::LoginReq
        );
        assert_eq!(
            DouyuMessageType::from_str("chatmsg"),
            DouyuMessageType::ChatMsg
        );
        assert_eq!(DouyuMessageType::from_str("dgb"), DouyuMessageType::Gift);
        assert_eq!(
            DouyuMessageType::from_str("unknown_type"),
            DouyuMessageType::Unknown
        );
    }

    #[test]
    fn test_create_login_message() {
        let msg = create_login_message("123456");
        assert!(msg.contains("type@=loginreq"));
        assert!(msg.contains("roomid@=123456"));
    }

    #[test]
    fn test_create_join_group_message() {
        let msg = create_join_group_message("123456", -9999);
        assert!(msg.contains("type@=joingroup"));
        assert!(msg.contains("rid@=123456"));
        assert!(msg.contains("gid@=-9999"));
    }

    #[test]
    fn test_parse_chat_message() {
        let payload =
            "type@=chatmsg/rid@=123456/uid@=user123/nn@=TestUser/txt@=Hello World!/level@=10/";
        let (msg_type, map) = parse_message(payload);

        assert_eq!(msg_type, DouyuMessageType::ChatMsg);

        let chat = DouyuChatMessage::from_map(&map).unwrap();
        assert_eq!(chat.nickname, "TestUser");
        assert_eq!(chat.content, "Hello World!");
        assert_eq!(chat.uid, "user123");
        assert_eq!(chat.level, 10);
        assert_eq!(chat.room_id, "123456");
    }

    #[test]
    fn test_parse_gift_message() {
        let payload = "type@=dgb/rid@=123456/uid@=user123/nn@=GiftUser/gfid@=1/gfname@=Rocket/gfcnt@=5/hits@=10/";
        let (msg_type, map) = parse_message(payload);

        assert_eq!(msg_type, DouyuMessageType::Gift);

        let gift = DouyuGiftMessage::from_map(&map).unwrap();
        assert_eq!(gift.nickname, "GiftUser");
        assert_eq!(gift.gift_id, "1");
        assert_eq!(gift.gift_name, "Rocket");
        assert_eq!(gift.gift_count, 5);
        assert_eq!(gift.hits, 10);
    }
}
