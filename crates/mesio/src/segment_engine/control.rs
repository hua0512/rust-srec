//! The control contract between a protocol's manifest side and the reactor.
//!
//! A protocol publishes manifest snapshots through a coalescing
//! `tokio::sync::watch` channel and supplies a [`SegmentPlanner`] that turns
//! each snapshot into a [`PlanOutcome`]. The reactor only ever sees the
//! outcome: descriptors to schedule, sequence ranges that will never arrive,
//! the window used for state pruning, consumer notices, and the terminal
//! cause. The terminal cause is carried on the snapshot, never inferred from
//! a sender drop, so a crashed watcher cannot masquerade as a clean end.

use std::sync::Arc;

use crate::DownloadError;

use super::descriptor::SegmentDescriptor;

/// Inclusive sequence-number range.
pub type SequenceRange = (u64, u64);

/// Why a manifest source stopped publishing snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCause {
    /// Authoritative end of the stream (HLS: `EXT-X-ENDLIST`). The snapshot
    /// carrying this still contains the final window and is planned before
    /// the reactor drains.
    End,
    /// The source could not keep refreshing (fetch/parse failure after
    /// retries). A pipeline error, never a clean end.
    Failed(Arc<str>),
    /// Preserve HTTP evidence across the watch channel after retries finish.
    HttpFailure {
        status: reqwest::StatusCode,
        url: Arc<str>,
        operation: &'static str,
    },
}

/// A sequence discontinuity that output cannot be continued across.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetKind {
    /// The window regressed too far below already-decided sequence numbers
    /// to be a stale edge response. Every re-based payload would sit below
    /// the assembler's emit cursor and be stale-rejected forever, so the
    /// stream must restart as a new session.
    SequenceRegressed,
}

/// The source's current window, bounding which finished state records may be
/// pruned (see `SegmentStateStore::prune_below`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneWindow {
    /// First sequence number still in the window.
    pub start: u64,
    /// Segments in the window; also bounds the download backlog kept below it.
    pub len: usize,
}

/// Consumer-facing notice derived from a planned snapshot. The assembler
/// forwards notices to its sink immediately, never reordered, so the
/// consumer channel keeps exactly one producer.
#[derive(Debug, Clone)]
pub enum EngineNotice {
    /// A snapshot was planned.
    ManifestRefreshed {
        sequence_base: u64,
        target_duration: f64,
    },
    /// The source declared its authoritative end.
    End,
}

/// What one planned snapshot means for the reactor.
#[derive(Debug, Default)]
pub struct PlanOutcome {
    /// Snapshot generation, for tracing.
    pub generation: u64,
    /// Every plannable segment in the window, including already-known keys:
    /// re-discovery refreshes volatile fetch metadata in the store.
    pub descriptors: Vec<SegmentDescriptor>,
    /// Sequence numbers proven gone before they were ever decided.
    pub missing: Vec<SequenceRange>,
    /// Sequence numbers that will never be planned (filtered or unusable).
    pub skipped: Vec<SequenceRange>,
    /// Window for state pruning; `None` leaves the store unpruned.
    pub prune: Option<PruneWindow>,
    /// Set when output cannot continue; the reactor fails the stream before
    /// ingesting anything from this outcome.
    pub reset: Option<ResetKind>,
    /// Notices forwarded after the skip ranges above, in order.
    pub notices: Vec<EngineNotice>,
    /// The snapshot's terminal cause. On `Failed`/`HttpFailure` the reactor
    /// ignores the rest of the outcome, so a planner need not plan it.
    pub terminal: Option<TerminalCause>,
}

/// A source failure the reactor surfaces as the stream's terminal error.
#[derive(Debug, Clone, Copy)]
pub enum SourceFailure<'a> {
    /// The snapshot carried `TerminalCause::Failed` with this reason.
    Failed(&'a str),
    /// The planner reported a reset.
    Reset(ResetKind),
    /// The snapshot sender closed without publishing a terminal cause: the
    /// watcher died.
    Lost,
}

/// Turns manifest snapshots into reactor control results. Owned by the
/// reactor, so cross-snapshot planning state needs no synchronization.
///
/// Contract: each media descriptor's `msn` is a per-stream ordinal. The
/// assembler emits in `msn` order, advances its cursor by one per segment,
/// rejects anything below the cursor as stale and treats a missing `msn` as a
/// gap. A protocol whose native numbering restarts or jumps (a new period,
/// time-based addressing) must map it to consecutive ordinals.
pub trait SegmentPlanner: Send + 'static {
    type Snapshot: Clone + Send + Sync + 'static;

    fn plan(&mut self, snapshot: &Self::Snapshot) -> PlanOutcome;

    /// The error a source failure surfaces as, in the protocol's terms.
    fn failure_error(&self, failure: SourceFailure<'_>) -> DownloadError;
}
