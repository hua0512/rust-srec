use crate::media::{StreamFormat, formats::MediaFormat};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StreamInfo {
    // Url of the stream
    pub url: String,
    // Name of the stream
    pub stream_format: StreamFormat,
    pub media_format: MediaFormat,
    // Quality of the stream, e.g., "1080p", "720p", etc.
    pub quality: String,
    // Bitrate of the stream in bits per second
    pub bitrate: u64,
    pub priority: u32,
    pub extras: Option<serde_json::Value>,
    pub codec: String,
    pub fps: f64,
    pub is_headers_needed: bool,
}

impl fmt::Display for StreamInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(extras) = &self.extras {
            if let Some(cdn) = extras.get("cdn").and_then(|v| v.as_str()) {
                write!(
                    f,
                    "{:?} ({}) - {} (CDN: {})",
                    self.stream_format, self.media_format, self.quality, cdn
                )
            } else {
                write!(
                    f,
                    "{:?} ({}) - {}",
                    self.stream_format, self.media_format, self.quality
                )
            }
        } else {
            write!(
                f,
                "{:?} ({}) - {}",
                self.stream_format, self.media_format, self.quality
            )
        }
    }
}
