#![allow(dead_code)]

use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct PandaTvBjResponse {
    pub result: bool,
    pub message: Option<String>,
    pub media: Option<PandaTvMedia>,
    #[serde(rename = "bjInfo")]
    pub bj_info: Option<PandaTvBjInfo>,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvMedia {
    pub title: String,
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "userIdx")]
    pub user_idx: i32,
    #[serde(rename = "userNick")]
    pub user_nick: String,
    #[serde(rename = "isAdult")]
    pub is_adult: bool,
    #[serde(rename = "isPw")]
    pub is_pw: bool,
    #[serde(rename = "isLive")]
    pub is_live: bool,
    #[serde(rename = "thumbUrl")]
    pub thumb_url: String,
    #[serde(rename = "userImg")]
    pub user_img: String,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvBjInfo {
    pub id: String,
    pub nick: String,
    #[serde(rename = "thumbUrl")]
    pub thumb_url: String,
    #[serde(rename = "channelTitle")]
    pub channel_title: String,
    #[serde(rename = "channelDesc")]
    pub channel_desc: String,
    #[serde(rename = "isBJ")]
    pub is_bj: String,
    #[serde(rename = "playTime")]
    pub play_time: PandaTvPlayTime,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvPlayTime {
    #[serde(rename = "monthTime")]
    pub month_time: i64,
    #[serde(rename = "totalTime")]
    pub total_time: i64,
    pub month: String,
    pub total: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PandaTvLiveResponse {
    // pub room_blind_info: PandaTvLiveRoomBlindInfo,
    // pub room_freeze_info: PandaTvLiveRoomFreezeInfo,
    pub token: String,
    pub enter_chat: Vec<serde_json::Value>,
    pub html5: bool,
    pub channel: String,
    pub media: PandaTvLiveMedia,
    pub mode: String,
    pub chat_mode: String,
    pub play_mode: String,
    #[serde(rename = "PlayList")]
    pub play_list: PandaTvLivePlayList,
    pub ie_play_mode: String,
    pub is_bookmark: bool,
    pub adult_out: bool,
    pub chat_server: PandaTvLiveChatServer,
    pub room_info: String,
    pub ranking: i32,
    pub chat_message: PandaTvLiveChatMessage,
    pub vip_deco: i32,
    pub result: bool,
    pub message: String,
    pub login_info: PandaTvLiveLoginInfo,
    // pub user_ip: String,
}

// #[derive(Deserialize, Debug)]
// #[serde(rename_all = "camelCase")]
// pub struct PandaTvLiveRoomBlindInfo {
//     pub enable: bool,
//     pub name: String,
//     pub blind_type: String,
// }

// #[derive(Deserialize, Debug)]
// #[serde(rename_all = "camelCase")]
// pub struct PandaTvLiveRoomFreezeInfo {
//     pub enable: bool,
//     pub name: String,
//     pub user_type: String,
//     pub manager_excpt: String,
//     pub fan_excpt: String,
// }

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PandaTvLiveMedia {
    pub code: String,
    pub title: String,
    pub user_id: String,
    pub user_idx: i32,
    pub user_nick: String,
    pub category: String,
    pub is_adult: bool,
    pub is_pw: bool,
    #[serde(rename = "type")]
    pub media_type: String,
    pub heart: i32,
    pub user: i32,
    pub user_limit: i32,
    pub size_width: i32,
    pub size_height: i32,
    pub browser: String,
    pub start_time: String,
    pub end_time: String,
    pub is_live: bool,
    pub on_air_type: String,
    pub live_type: String,
    pub play_cnt: i32,
    pub like_cnt: i32,
    pub bookmark_cnt: i32,
    pub fan_cnt: i32,
    pub total_score_cnt: i32,
    #[serde(rename = "newBjYN")]
    pub new_bj_yn: String,
    pub storage: String,
    #[serde(rename = "isGuestLive")]
    pub is_guest_live: String,
    pub thumb_url: String,
    pub user_img: String,
    pub list_up: String,
    pub list_deco: String,
    pub user_up: String,
    pub quality: i32,
    pub ivs_thumbnail: String,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLivePlayList {
    pub size: PandaTvLiveSize,
    pub hls: Vec<PandaTvLiveHlsStream>,
    pub hls2: Vec<PandaTvLiveHlsStream>,
    pub hls3: Vec<PandaTvLiveHlsStream>,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveSize {
    pub width: i32,
    pub height: i32,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveHlsStream {
    pub name: String,
    pub sort: i32,
    pub url: String,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveChatServer {
    pub url: String,
    pub t: i64,
    pub token: String,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveChatMessage {
    pub intro: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PandaTvLiveLoginInfo {
    pub sess_key: String,
    pub user_info: PandaTvLiveUserInfo,
    pub site_mode: PandaTvLiveSiteMode,
    pub device_info: PandaTvLiveDeviceInfo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PandaTvLiveUserInfo {
    pub is_login: bool,
    pub is_captcha: bool,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveSiteMode {
    #[serde(rename = "needAuth")]
    pub need_auth: bool,
    pub mode: String,
    #[serde(rename = "type")]
    pub site_mode_type: String,
}

#[derive(Deserialize, Debug)]
pub struct PandaTvLiveDeviceInfo {
    #[serde(rename = "type")]
    pub device_type: String,
    pub version: String,
}
