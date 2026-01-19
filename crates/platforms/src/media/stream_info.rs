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
///     let stream_info = StreamInfo::builder(
///         "https://example.com/stream",
///         StreamFormat::Hls,
///         MediaFormat::Mp4,
///     )
///     .quality("1080p")
///     .bitrate(5_000_000)
///     .priority(1)
///     .codec("h264")
///     .fps(30.0)
///     .build();
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
    /// Indicates if this stream contains only audio (no video)
    #[serde(default)]
    pub is_audio_only: bool,
}

#[derive(Debug, Clone)]
pub struct StreamInfoBuilder {
    url: String,
    stream_format: StreamFormat,
    media_format: MediaFormat,
    quality: String,
    bitrate: u64,
    priority: u32,
    extras: Option<serde_json::Value>,
    codec: String,
    fps: f64,
    is_headers_needed: bool,
    is_audio_only: bool,
}

impl StreamInfo {
    pub fn builder(
        url: impl Into<String>,
        stream_format: StreamFormat,
        media_format: MediaFormat,
    ) -> StreamInfoBuilder {
        StreamInfoBuilder::new(url, stream_format, media_format)
    }

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

    /// Returns a beautifully formatted multi-line string representation of the StreamInfo.
    ///
    /// # Arguments
    /// * `index` - The 1-based index of this stream in a list
    /// * `width` - The desired width for padding (use 0 for no padding)
    ///
    /// # Example
    ///
    /// ```rust
    /// use platforms_parser::media::{StreamInfo, StreamFormat, formats::MediaFormat};
    ///
    /// let stream = StreamInfo::builder("https://example.com/stream.m3u8", StreamFormat::Hls, MediaFormat::Ts)
    ///     .quality("1080p")
    ///     .bitrate(5_000_000)
    ///     .codec("h264")
    ///     .fps(30.0)
    ///     .build();
    ///
    /// println!("{}", stream.pretty_print(1, 60));
    /// ```
    pub fn pretty_print(&self, index: usize, width: usize) -> String {
        use std::fmt::Write;

        let mut output = String::new();

        // Stream header with markers
        let audio_marker = if self.is_audio_only { " 🔊" } else { "" };
        let priority_marker = if self.priority > 0 {
            format!(" [P:{}]", self.priority)
        } else {
            String::new()
        };
        let stream_header = format!(
            "  Stream #{}: {} | {}{}{}",
            index, self.quality, self.stream_format, audio_marker, priority_marker
        );
        if width > 0 {
            let padding = width.saturating_sub(stream_header.len());
            let _ = writeln!(output, "║{}{}║", stream_header, " ".repeat(padding));
        } else {
            let _ = writeln!(output, "{}", stream_header);
        }

        // Details line
        let codec_display = if self.codec.is_empty() {
            "N/A"
        } else {
            &self.codec
        };
        let details = format!(
            "    └─ Format: {} | Codec: {} | {}fps | {}kbps",
            self.media_format,
            codec_display,
            self.fps as u32,
            self.bitrate / 1000
        );
        if width > 0 {
            let details_padding = width.saturating_sub(details.len());
            let _ = writeln!(output, "║{}{}║", details, " ".repeat(details_padding));
        } else {
            let _ = writeln!(output, "{}", details);
        }

        // URL line (truncated if too long)
        let url_display = if self.url.len() > 50 {
            format!("{}...", &self.url[..50])
        } else {
            self.url.clone()
        };
        let url_line = format!("    └─ URL: {}", url_display);
        if width > 0 {
            let url_padding = width.saturating_sub(url_line.len());
            let _ = write!(output, "║{}{}║", url_line, " ".repeat(url_padding));
        } else {
            let _ = write!(output, "{}", url_line);
        }

        output
    }
}

impl StreamInfoBuilder {
    pub fn new(
        url: impl Into<String>,
        stream_format: StreamFormat,
        media_format: MediaFormat,
    ) -> Self {
        Self {
            url: url.into(),
            stream_format,
            media_format,
            quality: String::new(),
            bitrate: 0,
            priority: 0,
            extras: None,
            codec: String::new(),
            fps: 0.0,
            is_headers_needed: false,
            is_audio_only: false,
        }
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn quality(mut self, quality: impl Into<String>) -> Self {
        self.quality = quality.into();
        self
    }

    pub fn bitrate(mut self, bitrate: u64) -> Self {
        self.bitrate = bitrate;
        self
    }

    pub fn priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn extras(mut self, extras: serde_json::Value) -> Self {
        self.extras = Some(extras);
        self
    }

    pub fn extras_opt(mut self, extras: Option<serde_json::Value>) -> Self {
        self.extras = extras;
        self
    }

    pub fn codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = codec.into();
        self
    }

    pub fn fps(mut self, fps: f64) -> Self {
        self.fps = fps;
        self
    }

    pub fn is_headers_needed(mut self, is_headers_needed: bool) -> Self {
        self.is_headers_needed = is_headers_needed;
        self
    }

    pub fn is_audio_only(mut self, is_audio_only: bool) -> Self {
        self.is_audio_only = is_audio_only;
        self
    }

    pub fn build(self) -> StreamInfo {
        StreamInfo {
            url: self.url,
            stream_format: self.stream_format,
            media_format: self.media_format,
            quality: self.quality,
            bitrate: self.bitrate,
            priority: self.priority,
            extras: self.extras,
            codec: self.codec,
            fps: self.fps,
            is_headers_needed: self.is_headers_needed,
            is_audio_only: self.is_audio_only,
        }
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
