//! Check-record entity.
//!
//! A `CheckRecord` is the domain representation of one monitor poll outcome:
//! when it ran, how long it took, and what the system observed (live with a
//! selected stream, offline, filtered out by a schedule, fatal-errored, or
//! a transient extractor failure).
//!
//! Two layering responsibilities this type carries:
//!
//! 1. **Decouples persistence from runtime fan-out.** The check-history
//!    writer's MPSC and the WebSocket broadcaster both carry `CheckRecord`,
//!    not the storage-layer [`StreamerCheckHistoryDbModel`]. The HTTP/WS
//!    routes never reach into `database::models`; the storage type stays
//!    confined to repository boundaries.
//! 2. **Centralises classification.** The translation from a
//!    [`crate::monitor::LiveStatus`] to a `CheckOutcome` lives on the
//!    domain entity (`from_live_status`), so any future consumer (a
//!    different sink, a metrics emitter, a CLI) gets the same mapping
//!    without copy-pasting the variant walk.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::monitor::FatalErrorType;

/// Maximum length (in bytes) of a transient-error message after truncation.
/// Mirrored at the storage layer by
/// [`crate::database::models::streamer_check_history::MAX_ERROR_MESSAGE_LEN`]
/// to keep the domain and storage caps in sync.
pub const MAX_ERROR_MESSAGE_LEN: usize = 512;

/// Compact summary of the stream the selector picked for a live observation.
///
/// Deliberately omits the URL: stream URLs can carry signed query
/// parameters or CDN tokens we don't want surfacing in operator-visible
/// diagnostic surfaces (the check-history strip's tooltip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedStreamSummary {
    pub quality: String,
    pub stream_format: String,
    pub media_format: String,
    pub bitrate: u64,
    pub codec: String,
    pub fps: f64,
}

/// Reason a Filtered outcome was produced. Mirrors
/// [`crate::monitor::FilterReason`] but without the runtime-only
/// `next_available` schedule hint — the strip's tooltip only needs the
/// discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterCause {
    OutOfSchedule,
    TitleMismatch,
    CategoryMismatch,
}

impl FilterCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterCause::OutOfSchedule => "OutOfSchedule",
            FilterCause::TitleMismatch => "TitleMismatch",
            FilterCause::CategoryMismatch => "CategoryMismatch",
        }
    }
}

impl From<&crate::monitor::FilterReason> for FilterCause {
    fn from(reason: &crate::monitor::FilterReason) -> Self {
        use crate::monitor::FilterReason;
        match reason {
            FilterReason::OutOfSchedule { .. } => FilterCause::OutOfSchedule,
            FilterReason::TitleMismatch => FilterCause::TitleMismatch,
            FilterReason::CategoryMismatch => FilterCause::CategoryMismatch,
        }
    }
}

/// Outcome of a single monitor poll. Discriminated union — exactly one
/// variant per record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckOutcome {
    /// Streamer was live and a stream candidate was selected.
    Live {
        title: String,
        category: Option<String>,
        viewer_count: Option<u64>,
        /// Every candidate the platform extractor returned BEFORE selection
        /// narrowed the list to one. Empty only on degenerate live
        /// observations (the detector usually turns those into Offline
        /// first). The selected element is also referenced by
        /// `selected_stream` — duplicated rather than indexed so the
        /// `From<&CheckRecord> for StreamerCheckHistoryDbModel` projection
        /// stays one-shot per field.
        candidates: Vec<SelectedStreamSummary>,
        /// `None` only when the live observation produced zero candidates
        /// (rare; usually the detector turns this into Offline first).
        selected_stream: Option<SelectedStreamSummary>,
    },
    /// Streamer was not live.
    Offline,
    /// Streamer was live but a filter (schedule, keyword, category) suppressed it.
    Filtered {
        reason: FilterCause,
        title: String,
        category: Option<String>,
    },
    /// Platform reported a non-recoverable condition (banned, region-locked, …).
    FatalError { kind: FatalErrorType },
    /// Check itself failed before producing a status (network, parser, …).
    /// Message is truncated to [`MAX_ERROR_MESSAGE_LEN`] on a UTF-8 boundary.
    TransientError { message: String },
}

impl CheckOutcome {
    /// Stable string discriminator. Persisted in the DB and surfaced to
    /// clients; matches the migration's CHECK constraint values.
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckOutcome::Live { .. } => "live",
            CheckOutcome::Offline => "offline",
            CheckOutcome::Filtered { .. } => "filtered",
            CheckOutcome::FatalError { .. } => "fatal_error",
            CheckOutcome::TransientError { .. } => "transient_error",
        }
    }
}

/// One monitor poll outcome for a streamer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRecord {
    pub streamer_id: String,
    pub checked_at: DateTime<Utc>,
    /// Wall-clock time the check took. `Duration` rather than i64 ms so
    /// the domain stays unit-typed; storage flattens to ms.
    pub duration: Duration,
    pub outcome: CheckOutcome,
}

impl CheckRecord {
    /// Build a record from a successful `LiveStatus` observation.
    ///
    /// The classification (live/offline/filtered/fatal) is derived from
    /// the variant; the `duration` is taken on the caller's side because
    /// only the polling adapter knows when the check started.
    pub fn from_live_status(
        streamer_id: impl Into<String>,
        checked_at: DateTime<Utc>,
        duration: Duration,
        status: &crate::monitor::LiveStatus,
    ) -> Self {
        use crate::monitor::LiveStatus;

        let outcome = match status {
            LiveStatus::Live {
                title,
                category,
                viewer_count,
                streams,
                candidates,
                ..
            } => CheckOutcome::Live {
                title: title.clone(),
                category: category.clone(),
                viewer_count: *viewer_count,
                candidates: candidates.iter().map(SelectedStreamSummary::from).collect(),
                selected_stream: streams.first().map(SelectedStreamSummary::from),
            },
            LiveStatus::Offline => CheckOutcome::Offline,
            LiveStatus::Filtered {
                reason,
                title,
                category,
            } => CheckOutcome::Filtered {
                reason: FilterCause::from(reason),
                title: title.clone(),
                category: category.clone(),
            },
            // The fatal-variant guard catches every fatal `LiveStatus` value
            // via [`crate::monitor::LiveStatus::fatal_kind`]. Future
            // additions are routed here automatically.
            s if s.is_fatal_error() => CheckOutcome::FatalError {
                // `expect` is sound: `is_fatal_error` and `fatal_kind` are
                // defined together on `LiveStatus` and stay in sync — see
                // `monitor::detector` tests for the cross-check.
                kind: s
                    .fatal_kind()
                    .expect("fatal_kind() returns Some when is_fatal_error()"),
            },
            // Any future LiveStatus variant lands here as Offline — the
            // strip is best-effort and a missing variant must never panic.
            _ => CheckOutcome::Offline,
        };

        Self {
            streamer_id: streamer_id.into(),
            checked_at,
            duration,
            outcome,
        }
    }

    /// Build a transient-error record. Truncates the error message on a
    /// UTF-8 boundary so multibyte characters at the cap don't panic and
    /// log readers see a trailing ellipsis on clipped messages.
    pub fn from_error(
        streamer_id: impl Into<String>,
        checked_at: DateTime<Utc>,
        duration: Duration,
        error: &str,
    ) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            checked_at,
            duration,
            outcome: CheckOutcome::TransientError {
                message: truncate_utf8(error, MAX_ERROR_MESSAGE_LEN),
            },
        }
    }
}

impl From<&platforms_parser::media::StreamInfo> for SelectedStreamSummary {
    fn from(s: &platforms_parser::media::StreamInfo) -> Self {
        Self {
            quality: s.quality.clone(),
            stream_format: format!("{:?}", s.stream_format),
            media_format: format!("{:?}", s.media_format),
            bitrate: s.bitrate,
            codec: s.codec.clone(),
            fps: s.fps,
        }
    }
}

/// UTF-8-safe truncation with trailing ellipsis on clipping. Naive byte
/// slicing would panic mid-codepoint on multibyte characters.
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 1);
    out.push_str(&s[..end]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{FilterReason, LiveStatus};
    use platforms_parser::media::{StreamFormat, StreamInfo, formats::MediaFormat};

    fn stream() -> StreamInfo {
        StreamInfo {
            url: "https://example.com/stream.flv?signed=secret".to_string(),
            stream_format: StreamFormat::Flv,
            media_format: MediaFormat::Flv,
            quality: "best".to_string(),
            bitrate: 5_000_000,
            priority: 1,
            extras: None,
            codec: "h264".to_string(),
            fps: 30.0,
            is_headers_needed: false,
            is_audio_only: false,
        }
    }

    fn live(extracted: usize) -> LiveStatus {
        // Build `extracted` distinct candidate descriptors so we can assert
        // both the count round-trip and that each candidate's URL is
        // stripped from the persisted summary.
        let candidates: Vec<_> = (0..extracted).map(|_| stream()).collect();
        LiveStatus::Live {
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            started_at: None,
            viewer_count: Some(1234),
            avatar: None,
            streams: vec![stream()],
            media_headers: None,
            media_extras: None,
            next_check_hint: None,
            candidates,
        }
    }

    #[test]
    fn live_record_carries_telemetry_without_url() {
        let rec =
            CheckRecord::from_live_status("s1", Utc::now(), Duration::milliseconds(42), &live(5));
        assert_eq!(rec.outcome.as_str(), "live");
        let CheckOutcome::Live {
            candidates,
            selected_stream,
            title,
            ..
        } = &rec.outcome
        else {
            panic!("expected Live outcome");
        };
        assert_eq!(candidates.len(), 5);
        assert_eq!(title, "Playing Games");
        let s = selected_stream.as_ref().expect("selection present");
        assert_eq!(s.quality, "best");
        assert_eq!(s.bitrate, 5_000_000);
        // The URL must not survive the domain conversion — signed query
        // params and CDN tokens stay out of operator-visible surfaces.
        let json = serde_json::to_string(&rec).unwrap();
        assert!(!json.contains("signed=secret"));
    }

    #[test]
    fn offline_record_has_no_payload() {
        let rec = CheckRecord::from_live_status(
            "s1",
            Utc::now(),
            Duration::milliseconds(5),
            &LiveStatus::Offline,
        );
        assert_eq!(rec.outcome, CheckOutcome::Offline);
        assert_eq!(rec.outcome.as_str(), "offline");
    }

    #[test]
    fn filtered_record_carries_reason() {
        let rec = CheckRecord::from_live_status(
            "s1",
            Utc::now(),
            Duration::zero(),
            &LiveStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: None,
                },
                title: "Late".to_string(),
                category: None,
            },
        );
        let CheckOutcome::Filtered { reason, title, .. } = &rec.outcome else {
            panic!("expected Filtered outcome");
        };
        assert_eq!(*reason, FilterCause::OutOfSchedule);
        assert_eq!(title, "Late");
    }

    #[test]
    fn fatal_variants_set_kind_via_helper() {
        for (variant, expected) in [
            (LiveStatus::NotFound, FatalErrorType::NotFound),
            (LiveStatus::Banned, FatalErrorType::Banned),
            (LiveStatus::AgeRestricted, FatalErrorType::AgeRestricted),
            (LiveStatus::RegionLocked, FatalErrorType::RegionLocked),
            (LiveStatus::Private, FatalErrorType::Private),
            (
                LiveStatus::UnsupportedPlatform,
                FatalErrorType::UnsupportedPlatform,
            ),
        ] {
            let rec = CheckRecord::from_live_status("s1", Utc::now(), Duration::zero(), &variant);
            assert!(matches!(
                rec.outcome,
                CheckOutcome::FatalError { kind } if kind == expected
            ));
        }
    }

    #[test]
    fn transient_error_truncates_long_message() {
        let long = "x".repeat(MAX_ERROR_MESSAGE_LEN * 2);
        let rec = CheckRecord::from_error("s1", Utc::now(), Duration::milliseconds(1), &long);
        let CheckOutcome::TransientError { message } = rec.outcome else {
            panic!("expected TransientError outcome");
        };
        assert!(message.ends_with('…'));
        assert!(message.chars().count() <= MAX_ERROR_MESSAGE_LEN + 1);
    }

    #[test]
    fn transient_error_preserves_short_message() {
        let rec = CheckRecord::from_error("s1", Utc::now(), Duration::zero(), "boom");
        assert!(matches!(
            rec.outcome,
            CheckOutcome::TransientError { message } if message == "boom"
        ));
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        // 3-byte char repeated past the cap — naive truncation would slice
        // mid-codepoint and panic.
        let s = "字".repeat(MAX_ERROR_MESSAGE_LEN);
        let rec = CheckRecord::from_error("s1", Utc::now(), Duration::zero(), &s);
        let CheckOutcome::TransientError { message } = rec.outcome else {
            panic!("expected TransientError");
        };
        assert!(message.ends_with('…'));
    }
}
