#![expect(
    unused,
    reason = "API response models include fields not consumed by the extractor"
)]

use std::collections::HashMap;

use serde::Deserialize;

/// Response of `GET https://www.tiktok.com/api-live/user/room/`.
///
/// `data` is `null` (and `statusCode` non-zero) for unknown users; the SIGI
/// page state wraps the same `RoomUserInfo` under `LiveRoom.liveRoomUserInfo`.
#[derive(Debug, Deserialize)]
pub struct ApiLiveRoomResponse {
    #[serde(default)]
    pub data: Option<RoomUserInfo>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, rename = "statusCode")]
    pub status_code: i64,
}

/// Represents the top-level `SIGI_STATE` blob embedded in the live page HTML.
#[derive(Debug, Deserialize)]
pub struct TiktokResponse {
    /// Contains the main live room data.
    #[serde(rename = "LiveRoom")]
    pub live_room: Option<LiveRoom>,
}

/// Represents the main container for live room information.
#[derive(Debug, Deserialize)]
pub struct LiveRoom {
    #[serde(rename = "liveRoomStatus")]
    pub status: i32,

    /// Detailed information about the user and the stream.
    #[serde(rename = "liveRoomUserInfo")]
    pub user_info: Option<RoomUserInfo>,
}

/// Contains details about the user and the nested live room data.
#[derive(Debug, Deserialize)]
pub struct RoomUserInfo {
    /// The user who owns the live room.
    pub user: User,
    /// Contains stream-specific details.
    #[serde(rename = "liveRoom")]
    pub stream_details: Option<StreamDetails>,
}

/// Represents a TikTok user profile.
#[derive(Debug, Deserialize)]
pub struct User {
    /// The user's unique ID.
    #[serde(default)]
    pub id: String,
    /// The user's display name.
    #[serde(default)]
    pub nickname: String,
    /// The user's unique handle.
    #[serde(default, rename = "uniqueId")]
    pub unique_id: String,
    /// The user's profile signature or bio.
    #[serde(default)]
    pub signature: String,
    /// URL for the user's larger avatar.
    #[serde(default, rename = "avatarLarger")]
    pub avatar_larger: String,
    /// URL for the user's medium avatar.
    #[serde(default, rename = "avatarMedium")]
    pub avatar_medium: String,
    /// URL for the user's thumbnail avatar.
    #[serde(default, rename = "avatarThumb")]
    pub avatar_thumb: String,
    /// The secure user ID.
    #[serde(default, rename = "secUid")]
    pub sec_uid: String,
    /// Indicates if the user's account is secret.
    #[serde(default)]
    pub secret: bool,
    /// Indicates if the user is verified.
    #[serde(default)]
    pub verified: bool,
    /// Live status of the user: 2 = live, 4 = offline.
    #[serde(default)]
    pub status: i32,
    /// The webcast room ID (also the danmu room ID); may be numeric or string.
    #[serde(default, rename = "roomId", deserialize_with = "de_string_or_number")]
    pub room_id: String,
    /// The follow status in relation to the viewer.
    #[serde(default, rename = "followStatus")]
    pub follow_status: i32,
}

/// Contains detailed information about the live stream.
#[derive(Debug, Deserialize)]
pub struct StreamDetails {
    /// The title of the live stream.
    #[serde(default)]
    pub title: String,
    /// The URL for the stream's cover image.
    #[serde(default, rename = "coverUrl")]
    pub cover_url: String,
    /// The Unix timestamp (seconds) of when the stream started.
    #[serde(default, rename = "startTime", deserialize_with = "de_i64_or_string")]
    pub start_time: i64,
    /// The status of the stream.
    #[serde(default)]
    pub status: i32,
    /// Data for the primary (H.264/AVC) stream.
    #[serde(rename = "streamData")]
    pub stream_data: Option<StreamData>,
    /// Data for the HEVC (H.265) stream, if available.
    #[serde(rename = "hevcStreamData")]
    pub hevc_stream_data: Option<StreamData>,
}

/// Contains the pull data for a video stream, including different qualities.
#[derive(Debug, Deserialize)]
pub struct StreamData {
    /// The raw pull data containing stream URLs and options.
    #[serde(rename = "pull_data")]
    pub pull_data: PullData,
}

/// Holds the stream data string and available quality options.
#[derive(Debug, Deserialize)]
pub struct PullData {
    /// A list of available quality options for the stream.
    #[serde(default)]
    pub options: PullDataOptions,
    /// A JSON string containing detailed stream data.
    #[serde(default, rename = "stream_data")]
    pub stream_data: String,
}

/// Represents the available qualities for a stream.
#[derive(Debug, Default, Deserialize)]
pub struct PullDataOptions {
    /// A vector of qualities.
    #[serde(default)]
    pub qualities: Vec<QualityInfo>,
}

/// Describes a single stream quality.
#[derive(Debug, Deserialize)]
pub struct QualityInfo {
    /// The name of the quality (e.g., "Original", "720p").
    #[serde(default)]
    pub name: String,
    /// The SDK key associated with this quality (e.g., "origin", "hd").
    #[serde(default, rename = "sdk_key")]
    pub sdk_key: String,
    /// Higher is better; 0 when the platform omits it.
    #[serde(default)]
    pub level: i32,
}

/// Parsed from the `stream_data` JSON string, this holds the actual stream URLs.
#[derive(Debug, Deserialize)]
pub struct StreamDataInfo {
    /// A map from quality key to stream information.
    #[serde(default)]
    pub data: HashMap<String, StreamQualityInfo>,
}

/// Contains the main stream URL information for a given quality.
#[derive(Debug, Deserialize)]
pub struct StreamQualityInfo {
    /// The primary stream information.
    #[serde(rename = "main")]
    pub main_stream: StreamUrlInfo,
}

/// Contains the URLs for different streaming protocols (FLV, HLS).
#[derive(Debug, Deserialize)]
pub struct StreamUrlInfo {
    /// The FLV stream URL.
    #[serde(default)]
    pub flv: String,
    /// The HLS stream URL.
    #[serde(default)]
    pub hls: String,
    /// Additional SDK parameters, often as a JSON string.
    #[serde(default)]
    pub sdk_params: String,
}

/// Parsed from the `sdk_params` string, containing additional stream metadata.
#[derive(Debug, Default, Deserialize)]
pub struct SdkParams {
    /// The nominal video bitrate in bits per second.
    #[serde(default, rename = "vbitrate")]
    pub v_bitrate: u64,
    /// Video codec as reported by the platform ("h264" / "h265").
    #[serde(default, rename = "VCodec")]
    pub v_codec: String,
    /// Resolution as `WxH`; empty for audio-only streams.
    #[serde(default)]
    pub resolution: String,
    /// GOP length in seconds.
    #[serde(default)]
    pub gop: u32,
}

fn de_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(serde_json::Number),
    }

    Ok(match Option::<StringOrNumber>::deserialize(deserializer)? {
        Some(StringOrNumber::String(s)) => s,
        Some(StringOrNumber::Number(n)) => n.to_string(),
        None => String::new(),
    })
}

fn de_i64_or_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrString {
        Number(i64),
        String(String),
    }

    Ok(match Option::<NumberOrString>::deserialize(deserializer)? {
        Some(NumberOrString::Number(n)) => n,
        Some(NumberOrString::String(s)) => s.trim().parse().unwrap_or(0),
        None => 0,
    })
}
