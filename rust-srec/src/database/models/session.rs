//! Session and media output database models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Filter criteria for querying media outputs.
/// Requirements: 5.1, 5.2, 5.3, 5.4
#[derive(Debug, Clone, Default)]
pub struct OutputFilters {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by streamer ID (requires join with live_sessions).
    pub streamer_id: Option<String>,
}

impl OutputFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by session ID.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }
}

/// Filter criteria for querying sessions.
/// Requirements: 4.1, 4.3, 4.4, 4.5
#[derive(Debug, Clone, Default)]
pub struct SessionFilters {
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Filter sessions started after this date.
    pub from_date: Option<DateTime<Utc>>,
    /// Filter sessions started before this date.
    pub to_date: Option<DateTime<Utc>>,
    /// Filter for active sessions only (sessions without an end_time).
    pub active_only: Option<bool>,
}

impl SessionFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    /// Filter by date range.
    pub fn with_date_range(
        mut self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Self {
        self.from_date = from;
        self.to_date = to;
        self
    }

    /// Filter for active sessions only.
    pub fn with_active_only(mut self, active_only: bool) -> Self {
        self.active_only = Some(active_only);
        self
    }
}

/// Live session database model.
/// Represents a single, continuous live stream event.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct LiveSessionDbModel {
    pub id: String,
    pub streamer_id: String,
    /// ISO 8601 timestamp when the session began
    pub start_time: String,
    /// ISO 8601 timestamp when the session ended (null if ongoing)
    pub end_time: Option<String>,
    /// JSON array of timestamped stream titles
    pub titles: Option<String>,
    pub danmu_statistics_id: Option<String>,
    #[serde(default)]
    pub total_size_bytes: i64,
}

impl LiveSessionDbModel {
    pub fn new(streamer_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            streamer_id: streamer_id.into(),
            start_time: chrono::Utc::now().to_rfc3339(),
            end_time: None,
            titles: Some("[]".to_string()),
            danmu_statistics_id: None,
            total_size_bytes: 0,
        }
    }
}

/// Title entry for session titles JSON array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleEntry {
    /// ISO 8601 timestamp
    pub ts: String,
    pub title: String,
}

/// Media output database model.
/// Represents a single file generated during a live session.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MediaOutputDbModel {
    pub id: String,
    pub session_id: String,
    /// Self-referencing key for derived artifacts (e.g., thumbnail from video)
    pub parent_media_output_id: Option<String>,
    pub file_path: String,
    /// File type: VIDEO, AUDIO, THUMBNAIL, DANMU_XML
    pub file_type: String,
    pub size_bytes: i64,
    /// ISO 8601 timestamp of file creation
    pub created_at: String,
}

impl MediaOutputDbModel {
    pub fn new(
        session_id: impl Into<String>,
        file_path: impl Into<String>,
        file_type: MediaFileType,
        size_bytes: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            parent_media_output_id: None,
            file_path: file_path.into(),
            file_type: file_type.as_str().to_string(),
            size_bytes,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_media_output_id = Some(parent_id.into());
        self
    }
}

/// Media file types.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
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
}

/// Danmu statistics database model.
/// Aggregated statistics for danmu messages collected during a live session.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DanmuStatisticsDbModel {
    pub id: String,
    pub session_id: String,
    pub total_danmus: i64,
    /// JSON array of timestamp-and-count pairs
    pub danmu_rate_timeseries: Option<String>,
    /// JSON array of top 10 most active users
    pub top_talkers: Option<String>,
    /// JSON map of word frequencies
    pub word_frequency: Option<String>,
}

impl DanmuStatisticsDbModel {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            total_danmus: 0,
            danmu_rate_timeseries: Some("[]".to_string()),
            top_talkers: Some("[]".to_string()),
            word_frequency: Some("{}".to_string()),
        }
    }
}

/// Top talker entry for danmu statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTalkerEntry {
    pub user_id: String,
    pub username: String,
    pub message_count: i64,
}

/// Danmu rate entry for timeseries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DanmuRateEntry {
    /// ISO 8601 timestamp
    pub ts: String,
    pub count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_session_new() {
        let session = LiveSessionDbModel::new("streamer-1");
        assert_eq!(session.streamer_id, "streamer-1");
        assert!(session.end_time.is_none());
    }

    #[test]
    fn test_media_output_with_parent() {
        let output = MediaOutputDbModel::new(
            "session-1",
            "/path/to/video.mp4",
            MediaFileType::Video,
            1024,
        )
        .with_parent("parent-1");
        assert_eq!(output.parent_media_output_id, Some("parent-1".to_string()));
    }

    #[test]
    fn test_media_file_type() {
        assert_eq!(MediaFileType::Video.as_str(), "VIDEO");
        assert_eq!(
            MediaFileType::parse("THUMBNAIL"),
            Some(MediaFileType::Thumbnail)
        );
    }
}
