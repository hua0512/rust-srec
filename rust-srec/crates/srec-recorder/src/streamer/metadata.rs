//! Streamer metadata for in-memory storage.
//!
//! This module defines the lightweight metadata structure used for
//! in-memory streamer state management.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use srec_core::domain::{Priority, StreamerState, StreamerUrl};

pub use srec_core::domain::streamer::{
    DEFAULT_OFFLINE_CHECK_COUNT, DEFAULT_OFFLINE_CHECK_DELAY_MS,
};

/// Detect the built-in platform name for a validated streamer URL.
pub fn detect_platform_name(url: &StreamerUrl) -> Option<&'static str> {
    platforms_parser::extractor::factory::platform_name_for_url(url.as_str())
}
const MIN_DOWNLOAD_FAILURE_THRESHOLD: u32 = 2;

/// Derives the consecutive download-failure threshold from the effective
/// offline-check count.
///
/// The floor of two preserves the same transient-failure safety margin used
/// by the session offline classifier when operators configure a single
/// offline check.
pub(crate) fn download_failure_threshold(offline_check_count: u32) -> i32 {
    i32::try_from(offline_check_count.max(MIN_DOWNLOAD_FAILURE_THRESHOLD)).unwrap_or(i32::MAX)
}

/// Lightweight streamer metadata for in-memory storage.
///
/// This struct contains only the essential fields needed for
/// scheduling and state management, avoiding full entity hydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamerMetadata {
    /// Unique identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Stream URL.
    pub url: String,
    /// Platform configuration ID (e.g., "platform-douyin", "platform-twitch").
    pub platform_config_id: String,
    /// Template configuration ID (optional).
    pub template_config_id: Option<String>,
    /// Current state.
    pub state: StreamerState,
    /// Priority level.
    pub priority: Priority,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// Consecutive error count for backoff calculation.
    pub consecutive_error_count: i32,
    /// Disabled until timestamp (for exponential backoff).
    pub disabled_until: Option<Timestamp>,
    /// Last time the streamer was live.
    pub last_live_time: Option<Timestamp>,
    /// Last recorded error message.
    pub last_error: Option<String>,
    /// Streamer specific configuration override (JSON string).
    pub streamer_specific_config: Option<String>,
    /// Persisted platform metadata document. Only observations write it; ordinary edits keep it.
    #[serde(default, skip_serializing)]
    pub platform_metadata: Option<String>,
    /// Offline-check count after merging the Global → Platform → Template →
    /// Streamer configuration hierarchy.
    ///
    /// The serialized alias `effective_offline_check_count` is deprecated and
    /// will be removed in a future version.
    #[serde(
        default = "default_offline_check_count",
        alias = "effective_offline_check_count"
    )]
    pub offline_check_count: u32,
    /// Offline-check delay after applying the same configuration hierarchy as
    /// [`Self::offline_check_count`].
    ///
    /// The serialized alias `effective_offline_check_delay_ms` is deprecated
    /// and will be removed in a future version.
    #[serde(
        default = "default_offline_check_delay_ms",
        alias = "effective_offline_check_delay_ms"
    )]
    pub offline_check_delay_ms: u64,
    #[serde(default = "jiff::Timestamp::now")]
    pub created_at: Timestamp,
    #[serde(default = "jiff::Timestamp::now")]
    pub updated_at: Timestamp,
    /// When the streamer was deleted, mirroring `streamers.deleted_at`.
    ///
    /// Set while `ActorRegistry::retire_streamer` stands the streamer's
    /// runtime owners down and cleared only by the row leaving the database.
    /// [`Self::is_active`] reads it, which is the single point every owner
    /// consults.
    #[serde(default)]
    pub deleted_at: Option<Timestamp>,
}

fn default_offline_check_count() -> u32 {
    DEFAULT_OFFLINE_CHECK_COUNT
}

fn default_offline_check_delay_ms() -> u64 {
    DEFAULT_OFFLINE_CHECK_DELAY_MS
}

impl StreamerMetadata {
    /// Create new metadata from database model.
    ///
    /// `offline_check_*` are populated from defaults; resolve them via
    /// [`Self::apply_resolved_config`] once the merged config is known.
    pub fn from_db_model(model: &srec_sqlite::models::StreamerDbModel) -> Self {
        Self {
            id: model.id.clone(),
            name: model.name.clone(),
            url: model.url.clone(),
            platform_config_id: model.platform_config_id.clone(),
            template_config_id: model.template_config_id.clone(),
            state: StreamerState::parse(&model.state).unwrap_or_default(),
            priority: Priority::parse(&model.priority).unwrap_or_default(),
            avatar_url: model.avatar.clone(),
            consecutive_error_count: model.consecutive_error_count.unwrap_or(0),
            // Map the last_error from the DB model
            last_error: model.last_error.clone(),
            // Map config overrides
            streamer_specific_config: model.streamer_specific_config.clone(),
            platform_metadata: model.platform_metadata.clone(),
            offline_check_count: default_offline_check_count(),
            offline_check_delay_ms: default_offline_check_delay_ms(),
            disabled_until: model.disabled_until.map(srec_sqlite::time::ms_to_datetime),
            last_live_time: model.last_live_time.map(srec_sqlite::time::ms_to_datetime),
            created_at: srec_sqlite::time::ms_to_datetime(model.created_at),
            updated_at: srec_sqlite::time::ms_to_datetime(model.updated_at),
            deleted_at: model.deleted_at.map(srec_sqlite::time::ms_to_datetime),
        }
    }

    /// Refresh the cached offline-check values from a freshly resolved
    /// [`crate::config::MergedConfig`]. Called at registration
    /// and from `ActorRegistry`'s config-reconciliation path.
    pub fn apply_resolved_config(&mut self, merged: &crate::config::MergedConfig) {
        self.offline_check_count = merged.offline_check_count;
        self.offline_check_delay_ms = merged.offline_check_delay_ms;
    }

    /// Check if the streamer is currently disabled (in backoff).
    pub fn is_disabled(&self) -> bool {
        self.disabled_until
            .map(|until| Timestamp::now() < until)
            .unwrap_or(false)
    }

    /// Whether the streamer has been deleted and is waiting to be reaped.
    ///
    /// The row still exists — `StreamerManager::reap_deleted` removes it once
    /// `ActorRegistry::retire_streamer` reports the runtime owners have
    /// stopped — but nothing may treat it as a streamer the user still has.
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Check if the streamer is in an active state (can be checked/recorded).
    ///
    /// False for a deleted streamer whatever its `state`, which is what makes
    /// `ActorRegistry` stop spawning/running new work for it and `startup_task`
    /// refuse new downloads — the whole runtime stands down off this one
    /// predicate.
    pub fn is_active(&self) -> bool {
        !self.is_deleted()
            && matches!(
                self.state,
                StreamerState::NotLive
                    | StreamerState::Live
                    | StreamerState::OutOfSchedule
                    | StreamerState::InspectingLive
                    | StreamerState::TemporalDisabled
            )
    }

    /// Get the remaining backoff duration, if any.
    pub fn remaining_backoff(&self) -> Option<jiff::SignedDuration> {
        self.disabled_until.and_then(|until| {
            let now = Timestamp::now();
            if until > now {
                Some(until.duration_since(now))
            } else {
                None
            }
        })
    }

    /// [`Self::remaining_backoff`] as a `std::time::Duration`, for callers
    /// scheduling timers or filling wire payloads. `None` when no backoff
    /// is active.
    pub fn remaining_backoff_std(&self) -> Option<std::time::Duration> {
        self.remaining_backoff()
            .and_then(|duration| duration.try_into().ok())
    }

    /// Get the platform name derived from platform_config_id.
    ///
    /// Returns the platform name (e.g., "douyin", "twitch", "bilibili").
    /// Strips the "platform-" prefix from platform_config_id if present.
    pub fn platform(&self) -> &str {
        self.platform_config_id
            .strip_prefix("platform-")
            .unwrap_or(&self.platform_config_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_failure_threshold_tracks_offline_check_count_with_safety_floor() {
        assert_eq!(download_failure_threshold(1), 2);
        assert_eq!(download_failure_threshold(3), 3);
        assert_eq!(download_failure_threshold(5), 5);
        assert_eq!(download_failure_threshold(u32::MAX), i32::MAX);
    }

    /// The name is matched against `platform_config.platform_name` and decides which row a new
    /// streamer is stored under, so a URL must keep resolving to the same name.
    #[test]
    fn each_builtin_platform_url_reports_its_platform_name() {
        let cases = [
            ("https://www.huya.com/660000", Some("Huya")),
            ("https://live.douyin.com/123456789", Some("Douyin")),
            ("https://www.douyu.com/9999", Some("Douyu")),
            ("https://www.pandalive.co.kr/play/someone", Some("PandaTV")),
            ("https://weibo.com/u/1234567890", Some("Weibo")),
            ("https://www.twitch.tv/someone", Some("Twitch")),
            ("https://xhslink.com/m/844vKmW30jz", Some("RedBook")),
            ("https://live.bilibili.com/4063253", Some("Bilibili")),
            ("https://picarto.tv/someone", Some("Picarto")),
            ("https://www.tiktok.com/@someone/live", Some("TikTok")),
            ("https://twitcasting.tv/someone", Some("Twitcasting")),
            ("https://live.acfun.cn/live/123456", Some("Acfun")),
            ("https://play.sooplive.co.kr/someone/123", Some("SOOP")),
            ("https://www.bigo.tv/123456", Some("Bigo")),
            ("https://WWW.TWITCH.TV/Someone/", Some("Twitch")),
            ("https://LIVE.BILIBILI.COM/4063253", Some("Bilibili")),
            ("https://Play.SoopLive.co.kr/someone", Some("SOOP")),
            ("https://XHSLINK.COM/o/share123", Some("RedBook")),
            // A link to another platform in the query keeps the URL's own platform.
            (
                "https://www.douyu.com/9999?from=https://live.bilibili.com/4063253",
                Some("Douyu"),
            ),
            ("https://www.xiaohongshu.com/user/profile/6260c44f", None),
            ("https://example.com/someone", None),
        ];

        for (url, expected) in cases {
            let parsed = StreamerUrl::new(url).unwrap();
            assert_eq!(detect_platform_name(&parsed), expected, "{url}");
        }
    }
}
