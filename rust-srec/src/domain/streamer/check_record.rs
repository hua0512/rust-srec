//! Domain representation of one monitor poll outcome.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Maximum length in bytes of a persisted transient-error message.
pub const MAX_ERROR_MESSAGE_LEN: usize = 512;

/// A non-recoverable platform condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FatalErrorType {
    NotFound,
    Banned,
    AgeRestricted,
    RegionLocked,
    Private,
    UnsupportedPlatform,
}

impl FatalErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::Banned => "Banned",
            Self::AgeRestricted => "AgeRestricted",
            Self::RegionLocked => "RegionLocked",
            Self::Private => "Private",
            Self::UnsupportedPlatform => "UnsupportedPlatform",
        }
    }
}

/// Compact stream descriptor that deliberately excludes source URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedStreamSummary {
    pub quality: String,
    pub stream_format: String,
    pub media_format: String,
    pub bitrate: u64,
    pub codec: String,
    pub fps: f64,
}

/// Reason a filtered observation was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterCause {
    OutOfSchedule,
    TitleMismatch,
    CategoryMismatch,
}

impl FilterCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OutOfSchedule => "OutOfSchedule",
            Self::TitleMismatch => "TitleMismatch",
            Self::CategoryMismatch => "CategoryMismatch",
        }
    }
}

/// Outcome of a single monitor poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckOutcome {
    Live {
        title: String,
        category: Option<String>,
        viewer_count: Option<u64>,
        candidates: Vec<SelectedStreamSummary>,
        selected_stream: Option<SelectedStreamSummary>,
    },
    Offline,
    Filtered {
        reason: FilterCause,
        title: String,
        category: Option<String>,
    },
    FatalError {
        kind: FatalErrorType,
    },
    TransientError {
        message: String,
    },
}

impl CheckOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Live { .. } => "live",
            Self::Offline => "offline",
            Self::Filtered { .. } => "filtered",
            Self::FatalError { .. } => "fatal_error",
            Self::TransientError { .. } => "transient_error",
        }
    }
}

/// One timestamped monitor poll outcome for a streamer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRecord {
    pub streamer_id: String,
    pub checked_at: DateTime<Utc>,
    pub duration: Duration,
    pub outcome: CheckOutcome,
}

impl CheckRecord {
    pub fn new(
        streamer_id: impl Into<String>,
        checked_at: DateTime<Utc>,
        duration: Duration,
        outcome: CheckOutcome,
    ) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            checked_at,
            duration,
            outcome,
        }
    }

    pub fn from_error(
        streamer_id: impl Into<String>,
        checked_at: DateTime<Utc>,
        duration: Duration,
        error: &str,
    ) -> Self {
        Self::new(
            streamer_id,
            checked_at,
            duration,
            CheckOutcome::TransientError {
                message: crate::utils::text::truncate_bytes(error, MAX_ERROR_MESSAGE_LEN),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_error_truncates_long_message() {
        let long = "x".repeat(MAX_ERROR_MESSAGE_LEN * 2);
        let record = CheckRecord::from_error("s1", Utc::now(), Duration::zero(), &long);
        let CheckOutcome::TransientError { message } = record.outcome else {
            panic!("expected transient error");
        };

        assert!(message.ends_with('…'));
        assert!(message.len() <= MAX_ERROR_MESSAGE_LEN + '…'.len_utf8());
    }

    #[test]
    fn transient_error_preserves_utf8_boundaries() {
        let long = "字".repeat(MAX_ERROR_MESSAGE_LEN);
        let record = CheckRecord::from_error("s1", Utc::now(), Duration::zero(), &long);
        let CheckOutcome::TransientError { message } = record.outcome else {
            panic!("expected transient error");
        };

        assert!(message.ends_with('…'));
    }
}
