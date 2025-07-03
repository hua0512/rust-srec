use crate::media::MediaFormat;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt, sync::Arc};

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    pub extras: Option<Arc<HashMap<String, String>>>,
    pub codec: String,
    pub is_headers_needed: bool,
}



impl fmt::Display for StreamInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(extras) = &self.extras {
            if let Some(cdn) = extras.get("cdn") {
            write!(f, "{:?} - {} (CDN: {})", self.format, self.quality, cdn)
            } else {
            write!(f, "{:?} - {}", self.format, self.quality)
            }
        } else {
            write!(f, "{:?} - {}", self.format, self.quality)
        }
    }
}