#![allow(unused)]
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwitchResponse {
    pub data: Data,
    // pub extensions: Extensions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub user_or_error: Option<UserOrError>,
    pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
    pub duration_milliseconds: u64,
    pub operation_name: String,
    pub request_id: String,
}

// Structs for UserOrError (from the first object)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOrError {
    pub id: String,
    pub login: String,
    pub display_name: String,
    pub primary_color_hex: String,
    #[serde(rename = "profileImageURL")]
    pub profile_image_url: String,
    pub stream: Option<Stream>,
    #[serde(rename = "__typename")]
    pub typename: String,
    // pub channel: Channel,
}

// Structs for User (from the second object)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub primary_color_hex: String,
    pub is_partner: bool,
    #[serde(rename = "profileImageURL")]
    pub profile_image_url: String,
    pub primary_team: Option<PrimaryTeam>,
    // pub squad_stream: Option<serde_json::Value>,
    pub channel: Channel,
    pub last_broadcast: Option<LastBroadcast>,
    pub stream: Option<Stream>,
    #[serde(rename = "__typename")]
    pub typename: String,
}

// Common structs used by both User and UserOrError
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: String,
    pub viewers_count: u64,
    #[serde(rename = "__typename")]
    pub typename: String,
    #[serde(rename = "type")]
    pub stream_type: Option<String>,
    pub created_at: Option<String>,
    pub game: Option<Game>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub name: String,
    #[serde(rename = "__typename")]
    pub typename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    #[serde(rename = "__typename")]
    pub typename: String,
    #[serde(rename = "self")]
    pub self_edge: Option<ChannelSelfEdge>,
    pub trailer: Option<Trailer>,
    pub chanlets: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSelfEdge {
    pub is_authorized: bool,
    pub restriction_type: Option<String>,
    #[serde(rename = "__typename")]
    pub typename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trailer {
    pub video: Option<serde_json::Value>,
    #[serde(rename = "__typename")]
    pub typename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryTeam {
    pub id: String,
    pub name: String,
    pub display_name: String,
    #[serde(rename = "__typename")]
    pub typename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastBroadcast {
    pub id: String,
    pub title: String,
    #[serde(rename = "__typename")]
    pub typename: String,
}
