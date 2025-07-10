#![allow(unused)]

use std::collections::HashMap;

use serde::Deserialize;

/// Represents the top-level response from the TikTok API.
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
    pub id: String,
    /// The user's display name.
    pub nickname: String,
    /// The user's unique handle.
    #[serde(rename = "uniqueId")]
    pub unique_id: String,
    /// The user's profile signature or bio.
    pub signature: String,
    /// URL for the user's larger avatar.
    #[serde(rename = "avatarLarger")]
    pub avatar_larger: String,
    /// URL for the user's medium avatar.
    #[serde(rename = "avatarMedium")]
    pub avatar_medium: String,
    /// URL for the user's thumbnail avatar.
    #[serde(rename = "avatarThumb")]
    pub avatar_thumb: String,
    /// The secure user ID.
    #[serde(rename = "secUid")]
    pub sec_uid: String,
    /// Indicates if the user's account is secret.
    pub secret: bool,
    /// Indicates if the user is verified.
    pub verified: bool,
    /// The current status of the user.
    pub status: i32,
    /// The ID of the user's room.
    #[serde(rename = "roomId")]
    pub room_id: String,
    /// The follow status in relation to the viewer.
    #[serde(rename = "followStatus")]
    pub follow_status: i32,
}

/// Contains detailed information about the live stream.
#[derive(Debug, Deserialize)]
pub struct StreamDetails {
    /// The title of the live stream.
    pub title: String,
    /// The URL for the stream's cover image.
    #[serde(rename = "coverUrl")]
    pub cover_url: String,
    /// The Unix timestamp of when the stream started.
    #[serde(rename = "startTime")]
    pub start_time: i64,
    /// The status of the stream.
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
    pub options: PullDataOptions,
    /// A JSON string containing detailed stream data.
    #[serde(rename = "stream_data")]
    pub stream_data: String,
}

/// Represents the available qualities for a stream.
#[derive(Debug, Deserialize)]
pub struct PullDataOptions {
    /// A vector of qualities.
    pub qualities: Vec<QualityInfo>,
}

/// Describes a single stream quality.
#[derive(Debug, Deserialize)]
pub struct QualityInfo {
    /// The name of the quality (e.g., "origin", "sd").
    pub name: String,
    /// The SDK key associated with this quality.
    #[serde(rename = "sdk_key")]
    pub sdk_key: String,
}

/// Parsed from the `stream_data` JSON string, this holds the actual stream URLs.
#[derive(Debug, Deserialize)]
pub struct StreamDataInfo {
    /// A map from quality name to stream information.
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
    pub flv: String,
    /// The HLS stream URL.
    pub hls: String,
    /// Additional SDK parameters, often as a JSON string.
    #[serde(default)]
    pub sdk_params: String,
}

/// Parsed from the `sdk_params` string, containing additional stream metadata.
#[derive(Debug, Deserialize)]
pub struct SdkParams {
    /// The video bitrate of the stream.
    #[serde(default, rename = "vbitrate")]
    pub v_bitrate: u64,
}
