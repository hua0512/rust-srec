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
