use serde::Deserialize;

/// `anchor/live` envelope. `data` is read only once `error_code` is 0,
/// because error bodies carry other shapes there (such as `[]`).
#[derive(Debug, Deserialize)]
pub struct LiveResponse {
    pub code: Option<i64>,
    #[serde(default)]
    pub error_code: i64,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct LiveData {
    pub item: Item,
    pub user_info: LiveUser,
}

#[derive(Debug, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub desc: String,
    pub status: i64,
    pub stream_info: Option<StreamInfo>,
}

#[derive(Debug, Deserialize)]
pub struct StreamInfo {
    pub pull: Pull,
}

#[derive(Debug, Deserialize)]
pub struct Pull {
    #[serde(default)]
    pub live_origin_flv_url: String,
    #[serde(default)]
    pub live_origin_hls_url: String,
}

#[derive(Debug, Deserialize)]
pub struct LiveUser {
    #[serde(default)]
    pub screen_name: String,
    #[serde(default)]
    pub profile_image_url: String,
}

/// m.weibo.cn `container/getIndex?type=uid` response.
#[derive(Debug, Deserialize)]
pub struct ContainerResponse {
    pub ok: i64,
    pub data: Option<ContainerData>,
}

#[derive(Debug, Deserialize)]
pub struct ContainerData {
    #[serde(rename = "userInfo")]
    pub user_info: Option<ContainerUser>,
}

#[derive(Debug, Deserialize)]
pub struct ContainerUser {
    pub screen_name: Option<String>,
    pub avatar_hd: Option<String>,
    pub profile_image_url: Option<String>,
    /// Profile buttons. While the user is live, one of them is the live
    /// avatar, whose `userInfo.avatar.oid` is the room's live id. Kept
    /// untyped because the buttons differ in shape.
    #[serde(default)]
    pub toolbar_menus: Vec<serde_json::Value>,
}
