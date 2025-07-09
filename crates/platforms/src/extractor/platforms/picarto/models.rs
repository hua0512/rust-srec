#![allow(unused)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PicartoResponse {
    pub channel: Option<Channel>,
    #[serde(rename = "getLoadBalancerUrl")]
    pub load_balancer: Option<LoadBalancer>,
    #[serde(rename = "getMultiStreams")]
    pub get_multi_streams: Option<GetMultiStreams>,
}

#[derive(Debug, Deserialize)]
pub struct Channel {
    pub online: bool,
    pub id: u64,
    pub adult: bool,
    pub avatar: String,
    pub name: String,
    pub private: bool,
    pub title: String,
    pub streaming: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoadBalancer {
    pub url: String,
    pub origin: String,
}

#[derive(Debug, Deserialize)]
pub struct GetMultiStreams {
    pub name: String,
    pub streaming: bool,
    pub online: bool,
    // pub viewers: u64,
    // pub multistream: bool,
    pub streams: Vec<Stream>,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct Stream {
    pub id: u64,
    pub user_id: u64,
    pub name: String,
    // pub account_type: String,
    pub avatar: String,
    pub offline_image: Option<String>,
    pub streaming: bool,
    pub adult: bool,
    pub multistream: bool,
    pub viewers: u64,
    pub hosted: bool,
    pub host: bool,
    pub following: bool,
    pub subscription: bool,
    pub online: bool,
    #[serde(rename = "channelId")]
    pub channel_id: u64,
    pub stream_name: String,
    pub color: String,
    pub webrtc: bool,
    pub subscription_enabled: bool,
    pub thumbnail_image: String,
}
