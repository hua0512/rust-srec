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
/// * `extras` - Optional key-value pairs for additional platform-specific metadata
///
/// # Examples
///
/// ```rust
/// use rustc_hash::FxHashMap;
/// use platforms_parser::media::media_info::MediaInfo;
///
/// let media = MediaInfo {
///     site_url: "https://example.com".to_string(),
///     title: "Sample Stream".to_string(),
///     artist: "Sample Artist".to_string(),
///     cover_url: Some("https://example.com/cover.jpg".to_string()),
///     artist_url: Some("https://example.com/artist".to_string()),
///     is_live: true,
///     streams: vec![],
///     extras: Some(FxHashMap::default()),
/// };
/// ```
pub struct MediaInfo {
    // Site of the media platform
    pub site_url: String,
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub artist_url: Option<String>,
    pub is_live: bool,
    pub streams: Vec<StreamInfo>,
    pub extras: Option<FxHashMap<String, String>>,
}

impl MediaInfo {
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
        extras: Option<FxHashMap<String, String>>,
    ) -> Self {
        Self {
            site_url,
            title,
            artist,
            cover_url,
            artist_url,
            is_live,
            streams,
            extras,
        }
    }

    pub fn empty() -> Self {
        Self {
            site_url: "".to_string(),
            title: "".to_string(),
            artist: "".to_string(),
            cover_url: None,
            artist_url: None,
            is_live: false,
            streams: vec![],
            extras: None,
        }
    }
}
