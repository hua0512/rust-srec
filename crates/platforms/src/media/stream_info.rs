use crate::media::MediaFormat;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
pub struct StreamInfo {
    // Url of the stream
    pub url: String,
    // Name of the stream
    pub format: MediaFormat,
    // Quality of the stream, e.g., "1080p", "720p", etc.
    pub quality: String,
    // Bitrate of the stream in bits per second
    pub bitrate: u32,
    pub priority: u32,
    pub extras: Option<HashMap<String, String>>,
    pub codec: String,
    pub is_headers_needed: bool,
}
