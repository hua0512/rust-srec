#![expect(
    dead_code,
    reason = "API response models include fields not consumed by the extractor"
)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct VisitorLoginResponse {
    pub result: i32,
    pub error_msg: Option<String>,
    // Present only when result == 0; error payloads omit these, so they must be
    // Option for deserialization to reach the result check in Acfun::extract.
    #[serde(rename = "userId")]
    pub user_id: Option<i64>,
    #[serde(rename = "acfun.api.visitor_st")]
    pub visitor_st: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StartPlayResponse {
    pub result: i32,
    pub data: Option<StartPlayData>,
    pub host: String,
}

#[derive(Debug, Deserialize)]
pub struct StartPlayData {
    #[serde(rename = "liveId")]
    pub live_id: String,
    pub caption: String,
    #[serde(rename = "videoPlayRes")]
    pub video_play_res: String,
    #[serde(rename = "liveStartTime")]
    pub live_start_time: i64,
}

#[derive(Debug, Deserialize)]
pub struct VideoPlayRes {
    #[serde(rename = "liveAdaptiveManifest")]
    pub live_adaptive_manifest: Vec<LiveAdaptiveManifest>,
}

#[derive(Debug, Deserialize)]
pub struct LiveAdaptiveManifest {
    #[serde(rename = "adaptationSet")]
    pub adaptation_set: AdaptationSet,
}

#[derive(Debug, Deserialize)]
pub struct AdaptationSet {
    pub representation: Vec<Representation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Representation {
    pub id: u32,
    pub url: String,
    pub bitrate: u32,
    pub quality_type: String,
    pub media_type: String,
    pub level: u32,
    pub name: String,
    pub hidden: bool,
    pub enable_adaptive: bool,
    #[serde(rename = "defaultSelect")]
    pub is_default: bool,
}
