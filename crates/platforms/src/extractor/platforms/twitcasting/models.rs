#![allow(unused)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TwitcastingData {
    pub movie: Movie,
    pub hls: Option<Hls>,
    pub fmp4: Option<Fmp4>,
    #[serde(rename = "tc-hls")]
    pub tc_hls: Option<StreamContainer>,
    pub llfmp4: Option<StreamContainer>,
    #[serde(rename = "llfmp4.h265")]
    pub llfmp4_h265: Option<StreamContainer>,
    pub webrtc: Option<Webrtc>,
}

#[derive(Debug, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub live: bool,
}

#[derive(Debug, Deserialize)]
pub struct Hls {
    pub host: String,
    pub proto: String,
    pub source: bool,
}

#[derive(Debug, Deserialize)]
pub struct Fmp4 {
    pub host: String,
    pub proto: String,
    pub source: bool,
    pub mobilesource: bool,
}

#[derive(Debug, Deserialize)]
pub struct StreamContainer {
    pub streams: Streams,
}

#[derive(Debug, Deserialize)]
pub struct Streams {
    // those are hls streams
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub low: Option<String>,
    #[serde(default)]
    pub high: Option<String>,

    // those are fmp4 streams
    #[serde(default)]
    pub mobilesource: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Webrtc {
    pub streams: Streams,
    pub app: App,
}

#[derive(Debug, Deserialize)]
pub struct App {
    pub mode: String,
    pub url: String,
}
