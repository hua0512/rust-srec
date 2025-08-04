use crate::media::{StreamFormat, formats::MediaFormat};
use serde::{Deserialize, Serialize};
use std::fmt;

/// StreamInfo represents a media stream with all its properties and metadata.
///
/// ## Serialization
///
/// StreamInfo implements Serde's Serialize and Deserialize traits, allowing it to be
/// easily converted to and from various formats:
///
/// ### JSON Serialization
/// ```rust
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     use serde_json;
///     use platforms_parser::media::{StreamInfo, StreamFormat, formats::MediaFormat};
///
///     // Create a sample StreamInfo
///     let stream_info = StreamInfo {
///         url: "https://example.com/stream".to_string(),
///         stream_format: StreamFormat::Hls,
///         media_format: MediaFormat::Mp4,
///         quality: "1080p".to_string(),
///         bitrate: 5000000,
///         priority: 1,
///         extras: None,
///         codec: "h264".to_string(),
///         fps: 30.0,
///         is_headers_needed: false,
///     };
///
///     // Serialize to JSON
///     let json_string = serde_json::to_string(&stream_info)?;
///     let pretty_json = serde_json::to_string_pretty(&stream_info)?;
///
///     // Deserialize from JSON
///     let stream_info: StreamInfo = serde_json::from_str(&json_string)?;
///     Ok(())
/// }
/// ```
///
/// ### Other formats
/// The struct can be serialized to any format supported by Serde, including:
/// - YAML (with serde_yaml)
/// - TOML (with toml)
/// - CBOR (with serde_cbor)
/// - MessagePack (with rmp-serde)
///
/// ### Field serialization notes
/// - `stream_format` and `media_format` are serialized as strings using their `as_str()` methods
/// - `extras` field is optional and will serialize as `null` when `None`
/// - All numeric fields maintain their precision during serialization
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

impl StreamInfo {
    /// Serialize the StreamInfo to a JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Serialize the StreamInfo to a pretty-formatted JSON string
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize a StreamInfo from a JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Convert to a serde_json::Value for flexible manipulation
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// Create from a serde_json::Value
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
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
