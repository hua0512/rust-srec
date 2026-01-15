use chrono::{DateTime, TimeZone, Utc};
use rustc_hash::FxHashMap;

use super::stream_info::StreamInfo;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
/// Represents comprehensive information about a media item from a streaming platform.
///
/// This struct contains metadata and streaming details for media content,
/// including both live and on-demand content from various media platforms.
///
/// # Fields
///
/// * `site_url` - The URL of the media platform where the content is hosted
/// * `title` - The title or name of the media content
/// * `artist` - The name of the content creator, performer, or channel
/// * `cover_url` - Optional URL to the media's cover image or thumbnail
/// * `artist_url` - Optional URL to the artist's or creator's profile/channel page
/// * `is_live` - Boolean flag indicating whether the content is a live stream
/// * `streams` - Vector of available stream information with different qualities/formats
/// * `headers` - Optional key-value pairs for HTTP headers (Cookie, User-Agent, Referer, etc.)
/// * `extras` - Optional key-value pairs for additional platform-specific metadata
///
/// # Examples
///
/// ```rust
/// use rustc_hash::FxHashMap;
/// use platforms_parser::media::media_info::MediaInfo;
///
/// let media = MediaInfo::builder("https://example.com", "Sample Stream", "Sample Artist")
///     .cover_url("https://example.com/cover.jpg")
///     .artist_url("https://example.com/artist")
///     .category("Gaming")
///     .is_live(true)
///     .streams(vec![])
///     .extras(FxHashMap::default())
///     .build();
/// ```
pub struct MediaInfo {
    // Site of the media platform
    pub site_url: String,
    pub title: String,
    pub artist: String,
    #[serde(default, deserialize_with = "deserialize_category")]
    pub category: Option<Vec<String>>,
    pub live_start_time: Option<DateTime<Utc>>,
    pub cover_url: Option<String>,
    pub artist_url: Option<String>,
    pub is_live: bool,
    pub streams: Vec<StreamInfo>,
    pub headers: Option<FxHashMap<String, String>>,
    pub extras: Option<FxHashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct MediaInfoBuilder {
    site_url: String,
    title: String,
    artist: String,
    category: Option<Vec<String>>,
    live_start_time: Option<DateTime<Utc>>,
    cover_url: Option<String>,
    artist_url: Option<String>,
    is_live: bool,
    streams: Vec<StreamInfo>,
    headers: Option<FxHashMap<String, String>>,
    extras: Option<FxHashMap<String, String>>,
}

impl MediaInfo {
    pub fn builder(
        site_url: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
    ) -> MediaInfoBuilder {
        MediaInfoBuilder::new(site_url, title, artist)
    }

    /// Creates a new `MediaInfo` instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        site_url: String,
        title: String,
        artist: String,
        cover_url: Option<String>,
        artist_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
        headers: Option<FxHashMap<String, String>>,
        extras: Option<FxHashMap<String, String>>,
    ) -> Self {
        Self {
            site_url,
            title,
            artist,
            category: None,
            live_start_time: None,
            cover_url,
            artist_url,
            is_live,
            streams,
            headers,
            extras,
        }
    }

    pub fn empty() -> Self {
        Self {
            site_url: "".to_string(),
            title: "".to_string(),
            artist: "".to_string(),
            category: None,
            live_start_time: None,
            cover_url: None,
            artist_url: None,
            is_live: false,
            streams: vec![],
            headers: None,
            extras: None,
        }
    }

    /// Serialize the MediaInfo to a JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Serialize the MediaInfo to a pretty-formatted JSON string
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize a MediaInfo from a JSON string
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

impl MediaInfoBuilder {
    pub fn new(
        site_url: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
    ) -> Self {
        Self {
            site_url: site_url.into(),
            title: title.into(),
            artist: artist.into(),
            category: None,
            live_start_time: None,
            cover_url: None,
            artist_url: None,
            is_live: false,
            streams: Vec::new(),
            headers: None,
            extras: None,
        }
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(vec![category.into()]);
        self
    }

    pub fn category_opt(mut self, category: Option<Vec<String>>) -> Self {
        self.category = category;
        self
    }

    pub fn category_one_opt(mut self, category: Option<String>) -> Self {
        self.category = category.map(|c| vec![c]);
        self
    }

    pub fn live_start_time(mut self, live_start_time: DateTime<Utc>) -> Self {
        self.live_start_time = Some(live_start_time);
        self
    }

    pub fn live_start_time_opt(mut self, live_start_time: Option<DateTime<Utc>>) -> Self {
        self.live_start_time = live_start_time;
        self
    }

    pub fn live_start_time_unix_seconds(mut self, unix_seconds: i64) -> Self {
        self.live_start_time = Utc.timestamp_opt(unix_seconds, 0).single();
        self
    }

    pub fn live_start_time_unix_millis(mut self, unix_millis: i64) -> Self {
        self.live_start_time = Utc.timestamp_millis_opt(unix_millis).single();
        self
    }

    pub fn live_start_time_unix(mut self, unix_timestamp: i64) -> Self {
        // Heuristic: "seconds since epoch" is currently ~1.7e9. Anything above
        // 1e10 is almost certainly milliseconds.
        if unix_timestamp.abs() > 10_000_000_000 {
            self.live_start_time = Utc.timestamp_millis_opt(unix_timestamp).single();
        } else {
            self.live_start_time = Utc.timestamp_opt(unix_timestamp, 0).single();
        }
        self
    }

    pub fn cover_url(mut self, cover_url: impl Into<String>) -> Self {
        self.cover_url = Some(cover_url.into());
        self
    }

    pub fn cover_url_opt(mut self, cover_url: Option<String>) -> Self {
        self.cover_url = cover_url;
        self
    }

    pub fn artist_url(mut self, artist_url: impl Into<String>) -> Self {
        self.artist_url = Some(artist_url.into());
        self
    }

    pub fn artist_url_opt(mut self, artist_url: Option<String>) -> Self {
        self.artist_url = artist_url;
        self
    }

    pub fn is_live(mut self, is_live: bool) -> Self {
        self.is_live = is_live;
        self
    }

    pub fn streams(mut self, streams: Vec<StreamInfo>) -> Self {
        self.streams = streams;
        self
    }

    pub fn headers(mut self, headers: FxHashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    pub fn headers_opt(mut self, headers: Option<FxHashMap<String, String>>) -> Self {
        self.headers = headers;
        self
    }

    pub fn extras(mut self, extras: FxHashMap<String, String>) -> Self {
        self.extras = Some(extras);
        self
    }

    pub fn extras_opt(mut self, extras: Option<FxHashMap<String, String>>) -> Self {
        self.extras = extras;
        self
    }

    pub fn build(self) -> MediaInfo {
        MediaInfo {
            site_url: self.site_url,
            title: self.title,
            artist: self.artist,
            category: self.category,
            live_start_time: self.live_start_time,
            cover_url: self.cover_url,
            artist_url: self.artist_url,
            is_live: self.is_live,
            streams: self.streams,
            headers: self.headers,
            extras: self.extras,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CategoryField {
    One(String),
    Many(Vec<String>),
}

fn deserialize_category<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<CategoryField>::deserialize(deserializer)?;
    Ok(opt.map(|v| match v {
        CategoryField::One(s) => vec![s],
        CategoryField::Many(v) => v,
    }))
}
