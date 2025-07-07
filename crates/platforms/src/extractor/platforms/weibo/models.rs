#![allow(unused)]
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct WeiboLiveInfo {
    pub code: i64,
    pub data: Data,
    pub error_code: i64,
    pub msg: String,
    pub result: bool,
}

#[derive(Debug, Deserialize)]
pub struct Data {
    pub item: Item,
    pub scheme: String,
    // pub subscribe_list: Vec<serde_json::Value>,
    pub user_info: UserInfo,
}

#[derive(Debug, Deserialize)]
pub struct Item {
    pub anchor_uid: i64,
    pub attitudes_count: i64,
    pub auto_stime: i64,
    pub auto_weibo_time: i64,
    pub comments_count: i64,
    pub cover: String,
    pub desc: String,
    pub following: i64,
    pub free_type: i64,
    pub height: i64,
    pub landscape: i64,
    pub liked: i64,
    pub live_id: String,
    pub live_type: i64,
    pub mid: String,
    pub reposts_count: i64,
    pub status: i64,
    pub stream_info: StreamInfo,
    pub watch_limit_info: WatchLimitInfo,
    pub width: i64,
}

#[derive(Debug, Deserialize)]
pub struct StreamInfo {
    pub pull: Pull,
}

#[derive(Debug, Deserialize)]
pub struct Pull {
    pub live_origin_flv_url: String,
    pub live_origin_hls_url: String,
}

#[derive(Debug, Deserialize)]
pub struct WatchLimitInfo {
    pub free_time: i64,
    pub pay_coin: i64,
    #[serde(rename = "type")]
    pub type_field: i64,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    pub avatar: String,
    pub avatar_hd: String,
    pub avatar_large: String,
    pub cover_image_phone: String,
    pub followers_count: i64,
    pub gender: String,
    pub id: i64,
    pub idstr: String,
    pub name: String,
    pub profile_image_url: String,
    pub screen_name: String,
    pub uid: i64,
    pub user_auth_type: i64,
    pub verified: i64,
    pub verified_reason: String,
    // pub verified_reason_url: String,
    // pub verified_source: String,
    // pub verified_source_url: String,
    // pub verified_trade: String,
    // pub verified_type: i64,
    // pub verified_type_ext: i64,
}
