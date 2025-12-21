#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DouyuRoomInfoResponse {
    pub error: u64,
    pub data: DouyuRoomInfoData,
}

#[derive(Debug, Deserialize)]
pub struct DouyuRoomInfoData {
    pub room_id: String,
    pub room_thumb: String,
    // pub cate_id: u64,
    pub cate_name: String,
    pub room_name: String,
    pub room_status: String,
    pub start_time: String,
    pub owner_name: String,
    pub avatar: String,
    // pub online: u64,
}

/// Response from the betard API (www.douyu.com/betard/{rid})
/// This API provides more detailed room information including VIP status
#[derive(Debug, Deserialize)]
pub struct DouyuBetardResponse {
    pub room: DouyuBetardRoom,
}

#[derive(Debug, Deserialize)]
pub struct DouyuBetardRoom {
    pub room_id: u64,
    #[serde(default, deserialize_with = "deserialize_string_or_any")]
    pub room_name: String,
    #[serde(default, deserialize_with = "deserialize_string_or_any")]
    pub owner_name: String,
    pub show_status: u64,
    #[serde(rename = "videoLoop")]
    pub video_loop: u64,
    #[serde(rename = "isVip")]
    pub is_vip: u64,
    #[serde(default, deserialize_with = "deserialize_string_or_any")]
    pub room_thumb: String,
    #[serde(default, deserialize_with = "deserialize_string_or_any")]
    pub avatar: String,
}

/// Custom deserializer that handles both string and non-string values
/// Returns empty string for non-string values
fn deserialize_string_or_any<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Visitor;

    struct StringOrAnyVisitor;

    impl<'de> Visitor<'de> for StringOrAnyVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or any value")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(String::new())
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(String::new())
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            // Skip all map entries and return empty string
            while (map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?).is_some() {}
            Ok(String::new())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            // Skip all sequence entries and return empty string
            while (seq.next_element::<serde::de::IgnoredAny>()?).is_some() {}
            Ok(String::new())
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(StringOrAnyVisitor)
}

/// Custom deserializer that handles both boolean and integer (0/1) values
/// Returns false for 0, true for any non-zero integer
fn deserialize_bool_or_int<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Visitor;

    struct BoolOrIntVisitor;

    impl<'de> Visitor<'de> for BoolOrIntVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a boolean or integer (0/1)")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v != 0)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v != 0)
        }
    }

    deserializer.deserialize_any(BoolOrIntVisitor)
}

/// Response from the interactive game API (www.douyu.com/api/interactive/web/v2/list)
/// Used to detect if a room is running an interactive game
#[derive(Debug, Deserialize)]
pub struct DouyuInteractiveGameResponse {
    pub error: i32,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl DouyuInteractiveGameResponse {
    /// Returns true if the room has active interactive games
    pub fn has_interactive_game(&self) -> bool {
        match &self.data {
            Some(data) => {
                // If data is an object with content, there's an interactive game
                // Empty object {} or null means no game
                if let Some(obj) = data.as_object() {
                    !obj.is_empty()
                } else if let Some(arr) = data.as_array() {
                    !arr.is_empty()
                } else {
                    false
                }
            }
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DouyuH5PlayResponse {
    pub error: i32,
    pub msg: String,
    pub data: Option<DouyuH5PlayData>,
}

#[derive(Debug, Deserialize)]
pub struct DouyuH5PlayData {
    pub room_id: u64,
    pub rtmp_cdn: String,
    pub rtmp_url: String,
    pub rtmp_live: String,
    #[serde(rename = "cdnsWithName")]
    pub cdns: Vec<CdnsWithName>,
    pub multirates: Vec<Multirates>,
}

#[derive(Debug, Deserialize)]
pub struct CdnsWithName {
    pub name: String,
    pub cdn: String,
    #[serde(rename = "isH265")]
    pub is_h265: bool,
    pub re_weight: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Multirates {
    pub name: String,
    pub rate: u64,
    #[serde(rename = "highBit")]
    pub high_bit: u64,
    pub bit: u64,
    #[serde(rename = "diamondFan")]
    pub diamond_fan: u64,
}

/// Response from the mobile API (m.douyu.com/api/room/ratestream)
/// Mobile tokens have looser validation and are useful for CDN switching
#[derive(Debug, Deserialize)]
pub struct DouyuMobilePlayResponse {
    pub code: i32,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub data: Option<DouyuMobilePlayData>,
}

#[derive(Debug, Deserialize)]
pub struct DouyuMobilePlayData {
    /// The stream URL with query parameters
    pub url: String,
    #[serde(default)]
    pub rate: u64,
}

/// Parsed stream information from a Douyu stream URL
#[derive(Debug, Clone)]
pub struct ParsedStreamInfo {
    /// The Tencent app name (e.g., "dyliveflv1", "dyliveflv3")
    pub tx_app_name: String,
    /// The stream ID (room ID with suffix)
    pub stream_id: String,
    /// Query parameters from the URL
    pub query_params: std::collections::HashMap<String, String>,
    /// The original host
    pub host: String,
}

/// CDN origin types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdnOrigin {
    /// Douyu self-built CDN
    Douyu,
    /// Tencent Cloud CDN
    Tencent,
    /// Huawei Cloud CDN
    Huawei,
    /// Unknown origin
    Unknown,
}

impl CdnOrigin {
    pub fn from_str(s: &str) -> Self {
        match s {
            "dy" => CdnOrigin::Douyu,
            "tct" => CdnOrigin::Tencent,
            "hw" => CdnOrigin::Huawei,
            _ => CdnOrigin::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CdnOrigin::Douyu => "dy",
            CdnOrigin::Tencent => "tct",
            CdnOrigin::Huawei => "hw",
            CdnOrigin::Unknown => "unknown",
        }
    }
}

// ==================== Fallback Authentication ====================

/// Response from the encryption key API (www.douyu.com/wgapi/livenc/liveweb/websec/getEncryption)
/// Used for JS-free fallback authentication
#[derive(Debug, Clone, Deserialize)]
pub struct DouyuEncryptionResponse {
    pub error: i32,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub data: Option<DouyuEncryptionData>,
}

/// Encryption data containing the key material for signing
#[derive(Debug, Clone, Deserialize)]
pub struct DouyuEncryptionData {
    /// Random string used in signing
    pub rand_str: String,
    /// Number of MD5 iterations
    pub enc_time: u32,
    /// Encryption key
    pub key: String,
    /// Whether this is a special key (affects salt generation)
    #[serde(deserialize_with = "deserialize_bool_or_int")]
    pub is_special: bool,
    /// Additional encrypted data to include in requests
    pub enc_data: String,
    /// CPP data (contains additional info)
    #[serde(default)]
    pub cpp: Option<serde_json::Value>,
}

/// Cached encryption key with expiration
#[derive(Debug, Clone)]
pub struct CachedEncryptionKey {
    pub data: DouyuEncryptionData,
    pub expire_at: u64,
    pub user_agent: String,
}

impl CachedEncryptionKey {
    /// Creates a new cached key with 24-hour expiration
    pub fn new(data: DouyuEncryptionData, user_agent: String) -> Self {
        let expire_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86400; // 24 hours

        Self {
            data,
            expire_at,
            user_agent,
        }
    }

    /// Checks if the key is still valid
    pub fn is_valid(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now < self.expire_at
    }
}

/// Result of fallback signing operation
#[derive(Debug, Clone)]
pub struct FallbackSignResult {
    /// The authentication signature
    pub auth: String,
    /// Timestamp used in signing
    pub ts: u64,
    /// Encrypted data to include in request
    pub enc_data: String,
}
