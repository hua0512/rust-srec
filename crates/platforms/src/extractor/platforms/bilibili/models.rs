#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RoomInfo {
    pub code: i32,
    pub message: String,
    pub data: Option<RoomInfoData>,
}

#[derive(Debug, Deserialize)]
pub struct RoomInfoData {
    pub room_info: Option<RoomInfoDetails>,
    pub anchor_info: Option<RoomInfoAnchorInfo>,
}

#[derive(Debug, Deserialize)]
pub struct RoomInfoDetails {
    pub uid: u64,
    pub room_id: u64,
    pub short_id: u64,
    pub title: String,
    pub cover: String,
    pub tags: String,
    pub background: String,
    pub live_status: u8,
    pub live_start_time: u64,
    pub lock_status: u8,
    pub lock_time: u64,
    pub live_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct RoomInfoAnchorInfo {
    pub base_info: RoomInfoAnchorInfoBaseInfo,
}

#[derive(Debug, Deserialize)]
pub struct RoomInfoAnchorInfoBaseInfo {
    pub uname: String,
    pub face: String,
}

#[derive(Debug, Deserialize)]
pub struct RoomPlayInfo {
    pub code: i32,
    pub message: String,
    pub ttl: i32,
    pub data: RoomPlayInfoData,
}

#[derive(Debug, Deserialize)]
pub struct RoomPlayInfoData {
    pub room_id: u64,
    pub short_id: u64,
    pub uid: u64,
    pub is_hidden: bool,
    pub is_locked: bool,
    pub is_portrait: bool,
    pub live_status: u8,
    pub hidden_till: i64,
    pub lock_till: i64,
    pub encrypted: bool,
    pub pwd_verified: bool,
    pub live_time: i64,
    pub room_shield: i32,
    // pub all_special_types: Vec<i32>,
    pub playurl_info: PlayUrlInfo,
}

#[derive(Debug, Deserialize)]
pub struct PlayUrlInfo {
    pub conf_json: String,
    pub playurl: PlayUrl,
}

#[derive(Debug, Deserialize)]
pub struct PlayUrl {
    pub cid: u64,
    pub g_qn_desc: Vec<GqnDesc>,
    pub stream: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
pub struct GqnDesc {
    pub qn: i32,
    pub desc: String,
    pub hdr_desc: String,
    pub attr_desc: Option<String>,
    pub hdr_type: i32,
    pub media_base_desc: Option<MediaBaseDesc>,
}

#[derive(Debug, Deserialize)]
pub struct MediaBaseDesc {
    pub detail_desc: Option<DetailDesc>,
    pub brief_desc: Option<BriefDesc>,
}

#[derive(Debug, Deserialize)]
pub struct DetailDesc {
    pub desc: String,
}

#[derive(Debug, Deserialize)]
pub struct BriefDesc {
    pub desc: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Stream {
    pub protocol_name: String,
    pub format: Vec<Format>,
}

#[derive(Debug, Deserialize)]
pub struct Format {
    pub format_name: String,
    pub codec: Vec<Codec>,
}

#[derive(Debug, Deserialize)]
pub struct Codec {
    pub codec_name: String,
    pub current_qn: i32,
    pub accept_qn: Vec<i32>,
    pub base_url: String,
    pub url_info: Vec<UrlInfo>,
    pub hdr_qn: Option<i32>,
    pub dolby_type: i32,
    pub attr_name: String,
    pub hdr_type: i32,
    pub drm: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UrlInfo {
    pub host: String,
    pub extra: String,
    pub stream_ttl: i32,
}
