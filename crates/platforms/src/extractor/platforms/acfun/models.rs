#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct VisitorLoginResponse {
    pub result: i32,
    #[serde(rename = "userId")]
    pub user_id: i64,
    #[serde(rename = "acfun.api.visitor_st")]
    pub visitor_st: String,
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
