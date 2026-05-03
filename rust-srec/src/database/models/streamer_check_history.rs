//! `streamer_check_history` table model.
//!
//! Append-only ring buffer of monitor poll outcomes per streamer. One row per
//! call to `MonitorStatusChecker::check_status`. Powers the check-history
//! strip on the streamer details page; see the migration
//! `20260503120000_add_streamer_check_history.sql` for column semantics and
//! the writer's retention policy.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// One row from the `streamer_check_history` table.
///
/// Field semantics mirror the migration. The `outcome` column is a
/// discriminator string constrained to one of:
/// `live` | `offline` | `filtered` | `transient_error` | `fatal_error`. The
/// CHECK constraint on the table catches typos at insert time.
///
/// `fatal_kind`, `filter_reason`, and `error_message` are mutually exclusive
/// outcome-detail fields — exactly zero or one is populated for any given
/// row, depending on `outcome`. `streams_extracted` and `stream_selected`
/// are populated only on `outcome = 'live'`.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct StreamerCheckHistoryDbModel {
    /// Surrogate key (auto-increment). Stable for a given DB; not stable
    /// across exports/imports.
    pub id: i64,
    pub streamer_id: String,
    /// Milliseconds since Unix epoch (UTC).
    pub checked_at: i64,
    /// Wall-clock duration of the check, in milliseconds.
    pub duration_ms: i64,
    /// One of the five outcome discriminators; the `CHECK` constraint on the
    /// table enforces this at insert time.
    pub outcome: String,
    /// Discriminator detail for `outcome = 'fatal_error'`:
    /// `NotFound` | `Banned` | `AgeRestricted` | `RegionLocked` | `Private`
    /// | `UnsupportedPlatform`. NULL otherwise.
    pub fatal_kind: Option<String>,
    /// Discriminator detail for `outcome = 'filtered'`:
    /// `OutOfSchedule` | `TitleMismatch` | `CategoryMismatch`. NULL otherwise.
    pub filter_reason: Option<String>,
    /// Truncated transient-error message (≤ 512 chars at write time). NULL
    /// when `outcome != 'transient_error'`.
    pub error_message: Option<String>,
    /// Count of stream candidates the platform extractor returned BEFORE
    /// selection narrowed the list to one. Zero on non-live outcomes and on
    /// filtered outcomes that short-circuit before extraction.
    pub streams_extracted: i64,
    /// JSON-encoded descriptor of the selected stream:
    /// `{ "quality": "...", "stream_format": "...", "media_format": "...",
    ///    "bitrate": N, "codec": "...", "fps": F }`. NULL on non-live outcomes.
    pub stream_selected: Option<String>,
    /// JSON-encoded list of every candidate the platform extractor returned
    /// before selection narrowed it to one. Same per-element shape as
    /// `stream_selected`. NULL on non-live outcomes and on rows persisted
    /// before the column was added (the tooltip falls back to "selected
    /// only" for those).
    pub streams_extracted_json: Option<String>,
    /// Snapshot of the live-side title for tooltip display. NULL on non-live
    /// outcomes.
    pub title: Option<String>,
    /// Snapshot of the live-side category for tooltip display. NULL on
    /// non-live outcomes.
    pub category: Option<String>,
    /// Snapshot of the live-side viewer count for tooltip display. NULL on
    /// non-live outcomes or when the platform doesn't report it.
    pub viewer_count: Option<i64>,
}

/// Outcome discriminator values, kept in one place to prevent typos drifting
/// between the writer, repository, API, and migration `CHECK` constraint.
pub mod outcome {
    pub const LIVE: &str = "live";
    pub const OFFLINE: &str = "offline";
    pub const FILTERED: &str = "filtered";
    pub const TRANSIENT_ERROR: &str = "transient_error";
    pub const FATAL_ERROR: &str = "fatal_error";

    /// Every accepted outcome value, in the order the migration's CHECK lists
    /// them. Used by tests to assert every variant survives a round-trip.
    pub const ALL: &[&str] = &[LIVE, OFFLINE, FILTERED, TRANSIENT_ERROR, FATAL_ERROR];
}

/// Maximum length (in bytes) of `error_message` at write time. Longer
/// messages are truncated with a UTF-8-safe boundary by the domain entity.
/// Mirrors [`crate::domain::streamer::MAX_ERROR_MESSAGE_LEN`] so the storage
/// cap and the domain cap stay in sync.
pub const MAX_ERROR_MESSAGE_LEN: usize = 512;

impl From<&crate::domain::streamer::CheckRecord> for StreamerCheckHistoryDbModel {
    /// Flatten a domain [`CheckRecord`] into the persisted row shape.
    ///
    /// The domain layer owns classification (`from_live_status`,
    /// `from_error`, the `CheckOutcome` discriminator); this conversion is
    /// the only place where typed domain values become string discriminators
    /// and JSON blobs. Keeping it in `From` rather than spreading projection
    /// logic across repositories preserves the single-direction layering
    /// (domain → storage) the rest of the codebase uses.
    ///
    /// [`CheckRecord`]: crate::domain::streamer::CheckRecord
    fn from(record: &crate::domain::streamer::CheckRecord) -> Self {
        use crate::domain::streamer::CheckOutcome;

        let mut row = Self {
            id: 0,
            streamer_id: record.streamer_id.clone(),
            checked_at: record.checked_at.timestamp_millis(),
            duration_ms: record.duration.num_milliseconds(),
            outcome: record.outcome.as_str().to_string(),
            fatal_kind: None,
            filter_reason: None,
            error_message: None,
            streams_extracted: 0,
            stream_selected: None,
            streams_extracted_json: None,
            title: None,
            category: None,
            viewer_count: None,
        };

        match &record.outcome {
            CheckOutcome::Live {
                title,
                category,
                viewer_count,
                candidates,
                selected_stream,
            } => {
                row.streams_extracted = candidates.len() as i64;
                row.title = Some(title.clone());
                row.category = category.clone();
                row.viewer_count = viewer_count.map(|v| v as i64);
                row.stream_selected = selected_stream.as_ref().map(serialize_summary);
                if !candidates.is_empty() {
                    // Serialize the whole Vec in one pass — `SelectedStreamSummary`
                    // derives `Serialize` and its on-the-wire shape (six fields,
                    // no URL) matches the persisted column. Avoids the per-element
                    // intermediate `serde_json::Value` tree the previous
                    // `json!`-macro path built. `unwrap_or_default()` is sound:
                    // `Serialize` on a struct with `String`/numeric fields cannot
                    // fail.
                    row.streams_extracted_json =
                        Some(serde_json::to_string(candidates).unwrap_or_default());
                }
            }
            CheckOutcome::Offline => {}
            CheckOutcome::Filtered {
                reason,
                title,
                category,
            } => {
                row.filter_reason = Some(reason.as_str().to_string());
                row.title = Some(title.clone());
                row.category = category.clone();
            }
            CheckOutcome::FatalError { kind } => {
                row.fatal_kind = Some(kind.as_str().to_string());
            }
            CheckOutcome::TransientError { message } => {
                row.error_message = Some(message.clone());
            }
        }

        row
    }
}

/// Serialize one candidate / selected stream descriptor to the JSON shape
/// stored in `stream_selected` and `streams_extracted_json`. Uses
/// `SelectedStreamSummary`'s derived `Serialize` directly — its struct
/// shape is exactly the wire shape, so we skip building an intermediate
/// `serde_json::Value`. The `unwrap_or_default()` is sound: serialization
/// of a struct with `String` and numeric fields cannot fail.
fn serialize_summary(s: &crate::domain::streamer::SelectedStreamSummary) -> String {
    serde_json::to_string(s).unwrap_or_default()
}
