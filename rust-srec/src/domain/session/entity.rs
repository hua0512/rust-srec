//! Session entities.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A timestamped title entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleEntry {
    pub ts: DateTime<Utc>,
    pub title: String,
}

impl TitleEntry {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            ts: Utc::now(),
            title: title.into(),
        }
    }
}

/// Live session entity representing a single, continuous live stream event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSession {
    pub id: String,
    pub streamer_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub titles: Vec<TitleEntry>,
    pub danmu_statistics_id: Option<String>,
}

impl LiveSession {
    /// Create a new live session.
    pub fn new(streamer_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            streamer_id: streamer_id.into(),
            start_time: Utc::now(),
            end_time: None,
            titles: Vec::new(),
            danmu_statistics_id: None,
        }
    }

    /// End the session.
    pub fn end(&mut self) {
        self.end_time = Some(Utc::now());
    }

    /// End the session at a specific time.
    pub fn end_at(&mut self, time: DateTime<Utc>) {
        self.end_time = Some(time);
    }

    /// Add a title to the session.
    pub fn add_title(&mut self, title: impl Into<String>) {
        self.titles.push(TitleEntry::new(title));
    }

    /// Check if the session is still active.
    pub fn is_active(&self) -> bool {
        self.end_time.is_none()
    }

    /// Get the duration of the session.
    pub fn duration(&self) -> Option<Duration> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// Get the current title (most recent).
    pub fn current_title(&self) -> Option<&str> {
        self.titles.last().map(|t| t.title.as_str())
    }

    /// Link danmu statistics to this session.
    pub fn link_danmu_statistics(&mut self, stats_id: impl Into<String>) {
        self.danmu_statistics_id = Some(stats_id.into());
    }
}

/// Media file types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaFileType {
    Video,
    Audio,
    Thumbnail,
    DanmuXml,
}

impl MediaFileType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Video => "VIDEO",
            Self::Audio => "AUDIO",
            Self::Thumbnail => "THUMBNAIL",
            Self::DanmuXml => "DANMU_XML",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "VIDEO" => Some(Self::Video),
            "AUDIO" => Some(Self::Audio),
            "THUMBNAIL" => Some(Self::Thumbnail),
            "DANMU_XML" => Some(Self::DanmuXml),
            _ => None,
        }
    }

    /// Get file extension for this type.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Video => "mp4",
            Self::Audio => "mp3",
            Self::Thumbnail => "jpg",
            Self::DanmuXml => "xml",
        }
    }
}

/// Media output entity representing a file generated during a live session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOutput {
    pub id: String,
    pub session_id: String,
    pub parent_media_output_id: Option<String>,
    pub file_path: String,
    pub file_type: MediaFileType,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

impl MediaOutput {
    /// Create a new media output.
    pub fn new(
        session_id: impl Into<String>,
        file_path: impl Into<String>,
        file_type: MediaFileType,
        size_bytes: u64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            parent_media_output_id: None,
            file_path: file_path.into(),
            file_type,
            size_bytes,
            created_at: Utc::now(),
        }
    }

    /// Create a derived media output (e.g., thumbnail from video).
    pub fn derived(
        parent: &MediaOutput,
        file_path: impl Into<String>,
        file_type: MediaFileType,
        size_bytes: u64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: parent.session_id.clone(),
            parent_media_output_id: Some(parent.id.clone()),
            file_path: file_path.into(),
            file_type,
            size_bytes,
            created_at: Utc::now(),
        }
    }

    /// Check if this is a derived output.
    pub fn is_derived(&self) -> bool {
        self.parent_media_output_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_session_new() {
        let session = LiveSession::new("streamer-1");
        assert!(session.is_active());
        assert!(session.titles.is_empty());
    }

    #[test]
    fn test_live_session_end() {
        let mut session = LiveSession::new("streamer-1");
        session.end();
        assert!(!session.is_active());
        assert!(session.duration().is_some());
    }

    #[test]
    fn test_live_session_titles() {
        let mut session = LiveSession::new("streamer-1");
        session.add_title("First title");
        session.add_title("Second title");

        assert_eq!(session.titles.len(), 2);
        assert_eq!(session.current_title(), Some("Second title"));
    }

    #[test]
    fn test_media_output_new() {
        let output = MediaOutput::new(
            "session-1",
            "/path/to/video.mp4",
            MediaFileType::Video,
            1024,
        );
        assert_eq!(output.file_type, MediaFileType::Video);
        assert!(!output.is_derived());
    }

    #[test]
    fn test_media_output_derived() {
        let video = MediaOutput::new(
            "session-1",
            "/path/to/video.mp4",
            MediaFileType::Video,
            1024,
        );
        let thumbnail =
            MediaOutput::derived(&video, "/path/to/thumb.jpg", MediaFileType::Thumbnail, 100);

        assert!(thumbnail.is_derived());
        assert_eq!(thumbnail.parent_media_output_id, Some(video.id));
    }

    #[test]
    fn test_media_file_type() {
        assert_eq!(MediaFileType::Video.extension(), "mp4");
        assert_eq!(MediaFileType::DanmuXml.extension(), "xml");
    }
}
