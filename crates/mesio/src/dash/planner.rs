//! The per-track DASH planner: MPD snapshots → segment descriptors.
//!
//! One planner runs per selected track inside that track's reactor. It
//! selects the track's Representation and issues available segments under
//! consecutive per-track ordinals (the reactor's `msn`), as the
//! [`SegmentPlanner`] contract requires: DASH segment numbers restart per
//! Period and are absent from time-based addressing, so they cannot be used
//! as sequence numbers.
//!
//! A segment's position within its Representation (its *locator*) is what
//! its URL is built from: its number under `$Number$` templates, with or
//! without a `SegmentTimeline`, and under any `@duration` addressing; its
//! media time under every other `SegmentTimeline` addressing (`$Time$`
//! templates, timed lists), where numbers are positions in the listed
//! timeline and shift when an MPD update drops expired `S` entries.
//!
//! MPD updates may contradict each other's timing: some origins move
//! `availabilityStartTime` with every update, rewrite the timeline's media
//! times, or describe segments far beyond the clock's live edge, while the
//! numbers stay consistent. Planning only ever moves forward by locator, so
//! such an update never re-plans or reorders work; one that offers nothing
//! after the newest planned segment is skipped. An epoch change within a
//! Period follows numbers within one Representation's `$Number$` timeline
//! (whatever its URLs, initialization or CDN), else media time while the
//! Period's timing is trusted, else starts like a recording
//! (`DashPlanner::switch_index`). The clock is overruled only after updates
//! contradicted the Period's timing, and then only for a `$Number$`
//! timeline of the Period the clock says is running, none of whose listed
//! segments the clock considers available ([`timeline_contradicts_clock`]).
//! Timing trust is kept per Period. Each kind of contradiction is logged
//! once per track. Output timing does not depend on any of this: the muxer
//! times output from the media (`tfdt`/`trun`); only the wall-clock anchor
//! is withdrawn once the timing was contradicted, also from work already
//! planned ([`WallClockAnchor`]).
//!
//! Known limitation: a numbering that restarts below the newest planned
//! number under the same Period and Representation ids (an encoder restart
//! that renumbers from 1) cannot be told from a stale MPD, so nothing is
//! planned until the numbers pass the frontier again. Unless they do soon,
//! the assembler's live stall watchdog ends the session with
//! `DownloadError::Timeout`, and a new recording starts from scratch.
//!
//! An *epoch* is one run of segments sharing one initialization segment and
//! timing: a Period and Representation, until the Representation's
//! initialization URL, byte range, timescale, presentation time offset or
//! addressing mode changes. Each epoch has its own identity scope, so a URL
//! reused by another epoch is a distinct resource, and its own init key, so
//! downstream sees a new initialization segment at every epoch change.
//!
//! Work is admitted through bounded windows, because a short MPD can
//! describe an unbounded number of segments and Periods:
//!
//! - at most [`PLANNING_LOOKAHEAD`] ordinals ahead of the *consumed
//!   frontier* (the next ordinal the track's assembler has not emitted or
//!   skipped, published through [`ConsumedFrontier`]), and at most that
//!   many per snapshot;
//! - at most one epoch beyond the one being consumed, so initialization
//!   segments (which download first) cannot run ahead of their media and be
//!   evicted before use.
//!
//! When an MPD update arrives, every planned segment not yet consumed is
//! issued again under its original ordinal and key with the URL from the
//! new MPD, so queued and retrying work picks up refreshed URLs (and signed
//! URL renewals reach the store's 401/403 handling).
//!
//! A Period that has begun without this track is reported downstream
//! (`EngineNotice::TrackAbsent`), so the muxer records the other track
//! through it and this track's stall detection stays quiet.
//!
//! Where recording starts is decided once per session ([`decide_start`]) and
//! shared by every track. A configured track that the starting Period lacks
//! is not recorded, even if a later Period carries it; the first such Period
//! is reported with a warning.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use tracing::{debug, info, warn};
use url::Url;

use crate::DownloadError;
use crate::dash::config::{DashSelectionConfig, LiveStartPolicy};
use crate::dash::manifest::{
    AddressingKind, ByteRange, Clock, Manifest, ManifestError, Period, PresentationType,
    RejectedContent, Representation, Segment, SegmentWindow, UnsupportedFeature,
};
use crate::dash::selection;
use crate::dash::watcher::MpdSnapshot;
use crate::error::SharedError;
use crate::segment_engine::control::{
    EngineNotice, PlanOutcome, PruneWindow, SegmentPlanner, SourceFailure, TerminalCause,
};
use crate::segment_engine::descriptor::{
    AnchorWithdrawal, DashSegmentMeta, MediaTrack, SegmentDescriptor, SegmentMeta, SegmentSource,
    WallClockAnchor, inherit_query_params,
};
use crate::segment_engine::identity::{
    ByteRangeKey, IdentityPolicy, SegmentIdentityPolicy, SegmentKey, SegmentKind,
};

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Ordinals planned ahead of the consumed frontier, and new segments planned
/// per snapshot, at most.
pub(crate) const PLANNING_LOOKAHEAD: u64 = 256;

/// The next ordinal a track's assembler has neither emitted nor skipped.
/// Written by the track's assembler sink, read by its planner.
#[derive(Debug, Default)]
pub(crate) struct ConsumedFrontier(AtomicU64);

impl ConsumedFrontier {
    pub(crate) fn advance_to(&self, next: u64) {
        self.0.fetch_max(next, Ordering::Relaxed);
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A Period's identity across MPD updates: `@id` when present, else its exact
/// start (ISO/IEC 23009-1 §5.3.2.1 makes `@id` the update-stable handle).
#[derive(Debug, Clone)]
pub(crate) struct PeriodKey {
    pub id: Option<Arc<str>>,
    pub start: Duration,
}

impl PeriodKey {
    pub(crate) fn of(period: &Period) -> Self {
        Self {
            id: period.id.as_deref().map(Arc::from),
            start: period.start,
        }
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            _ => self.start == other.start,
        }
    }

    /// Whether `self` comes at or after `other` in presentation order.
    fn at_or_after(&self, other: &Self) -> bool {
        self.same(other) || self.start > other.start
    }

    pub(crate) fn label(&self) -> String {
        match &self.id {
            Some(id) => format!("id:{id}"),
            None => format!("start:{}", self.start.as_nanos()),
        }
    }
}

/// Where a session starts recording: in `period`, every segment ending
/// after `after_nanos` (presentation time on the MPD timeline), or every
/// available segment when `None`.
#[derive(Debug, Clone)]
pub(crate) struct StartPosition {
    pub period: PeriodKey,
    pub after_nanos: Option<i128>,
}

/// What makes segments decodable with one initialization segment on one
/// timeline. The URL is compared without its query so rotating access
/// tokens do not split a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    init_location: String,
    init_range: Option<ByteRangeKey>,
    timescale: u64,
    presentation_time_offset: u64,
    timeline: bool,
    /// A segment's key depends on it: indexed segments are byte ranges of
    /// one resource, so work planned under another addressing cannot be
    /// reissued with the new URLs.
    addressing: AddressingKind,
}

impl Fingerprint {
    fn locator(&self) -> Locator {
        if self.timeline && self.addressing != AddressingKind::Number {
            Locator::Time
        } else {
            Locator::Number
        }
    }
}

/// What identifies a segment within its Representation (see the module
/// documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Locator {
    Number,
    Time,
}

/// A planned, not yet consumed segment.
#[derive(Debug)]
struct Planned {
    locator: u64,
    ordinal: u64,
    key: SegmentKey,
}

#[derive(Debug)]
struct Epoch {
    period: PeriodKey,
    representation_id: Arc<str>,
    fingerprint: Fingerprint,
    scope: Arc<str>,
    init_key: SegmentKey,
    init_meta: Arc<DashSegmentMeta>,
    first_ordinal: u64,
    planned: VecDeque<Planned>,
    last_locator: u64,
    /// Media time of the newest planned segment, as the MPD planning it
    /// described it.
    last_time: u64,
    /// Presentation end of the newest planned segment, in nanoseconds.
    last_end: i128,
    /// `MPD@availabilityStartTime` when the epoch opened.
    availability_start: Option<DateTime<Utc>>,
    /// `availability_start` as the wall-clock anchor every descriptor of
    /// the epoch shares, while its timing was not contradicted.
    anchor: Option<WallClockAnchor>,
}

/// What MPD updates showed about one Period's timing.
#[derive(Debug)]
struct PeriodTrust {
    period: PeriodKey,
    /// Whether updates contradicted its timing: the anchor moved, or a
    /// planned number was described at another time.
    contradicted: bool,
    /// Withdraws the wall-clock anchor of every epoch of the Period.
    withdrawal: AnchorWithdrawal,
}

/// A segment's stable identity within its Representation (see the module
/// documentation).
fn locate(segment: &Segment, locator: Locator) -> u64 {
    match locator {
        Locator::Number => segment.number,
        Locator::Time => segment.time,
    }
}

/// Slack, on top of one segment duration, by which even the earliest
/// segment a running Period's timeline lists may lie ahead of the clock's
/// live edge before the clock is overruled. A server may list the segment
/// it is producing, and the clock offset is only as precise as the `Date`
/// header (whole seconds) or time server it came from.
const TIMELINE_LEAD_SLACK: Duration = Duration::from_secs(10);

/// Segments of a Representation that are available, and when.
struct Availability<'a> {
    window: SegmentWindow<'a>,
    /// The instant (server time) the window was evaluated at.
    at: DateTime<Utc>,
    /// How far past the clock `at` lies, when the timeline contradicted it.
    lead: Option<Duration>,
}

/// How far ahead of the clock the newest segment `representation` lists
/// lies, when its timing anchor and its `SegmentTimeline` cannot both be
/// right.
///
/// An MPD may describe segments before they become available, so a
/// listing alone proves nothing: a running Period can legitimately list
/// only segments still to come. The caller therefore considers this only
/// once MPD updates have contradicted each other's timing for the Period
/// (see [`DashPlanner::contradicted`]), and it applies only when all of
/// these hold as well:
///
/// - The Period is running by the clock ([`Representation::timeline_ahead`]
///   ignores others), so a preannounced Period never becomes the live edge.
/// - Not even the earliest listed segment is available, by more than one
///   segment plus [`TIMELINE_LEAD_SLACK`]. A server keeps the segments it
///   has produced listed for the time-shift buffer, so a running Period's
///   timeline in which nothing has been produced yet means the anchor that
///   places it is wrong, not that the media does not exist.
/// - Its URLs are keyed by `$Number$`, which does not depend on the
///   contradicted times. A `$Time$` URL is built from the very times in
///   question, so nothing says those resources exist.
fn timeline_contradicts_clock(
    representation: &Representation,
    now: DateTime<Utc>,
) -> Result<Option<Duration>, ManifestError> {
    if representation.addressing_kind() != AddressingKind::Number {
        return Ok(None);
    }
    Ok(representation
        .timeline_ahead(now)?
        .filter(|ahead| ahead.earliest > ahead.segment_duration.saturating_add(TIMELINE_LEAD_SLACK))
        .map(|ahead| ahead.newest))
}

/// The segments of `representation` available at `now`. With `overrule`
/// (the Period's timing was contradicted), a timeline that contradicts the
/// clock ([`timeline_contradicts_clock`]) is evaluated at the instant its
/// newest listed segment becomes available instead, which keeps the
/// time-shift buffer relative to the listed segments.
fn available(
    representation: &Representation,
    now: DateTime<Utc>,
    overrule: bool,
) -> Result<Availability<'_>, ManifestError> {
    let lead = match overrule {
        true => timeline_contradicts_clock(representation, now)?,
        false => None,
    };
    let at = lead
        .and_then(|lead| TimeDelta::from_std(lead).ok())
        .and_then(|lead| now.checked_add_signed(lead));
    let (at, lead) = match at {
        Some(at) => (at, lead),
        None => (now, None),
    };
    Ok(Availability {
        window: representation.segments(at)?,
        at,
        lead,
    })
}

/// Ways an MPD update can contradict the timing of earlier ones, each
/// reported once per track.
#[derive(Debug, Clone, Copy)]
enum TimingConflict {
    /// `MPD@availabilityStartTime` changed.
    AnchorMoved = 1,
    /// The timeline describes segments far beyond the clock's live edge.
    AheadOfClock = 2,
    /// A segment's media time changed, or a later segment does not start
    /// after an earlier one.
    TimesRewritten = 4,
}

/// Whether a dynamic MPD's `availabilityStartTime` differs from the one
/// `epoch` opened with.
fn anchor_moved(manifest: &Manifest, epoch: &Epoch) -> bool {
    manifest.presentation_type == PresentationType::Dynamic
        && manifest.availability_start_time != epoch.availability_start
}

/// The Period and Representation being planned, with the snapshot they
/// come from.
#[derive(Clone, Copy)]
struct PeriodScope<'a> {
    snapshot: &'a MpdSnapshot,
    period: &'a Period,
    key: &'a PeriodKey,
    representation: &'a Representation,
    /// The instant (server time) the window was evaluated at.
    at: DateTime<Utc>,
    /// How far ahead of the clock the Representation's timeline was
    /// evaluated (see [`available`]).
    lead: Option<Duration>,
}

impl PeriodScope<'_> {
    fn manifest(&self) -> &Manifest {
        &self.snapshot.manifest
    }

    fn bounds(&self, segment: &Segment) -> (i128, i128) {
        segment_bounds(segment, self.representation, self.period)
    }
}

/// Why planning a Period stopped.
enum PeriodPlan {
    /// Every currently available segment is planned.
    Complete,
    /// A window bound was reached; more will be planned later.
    Blocked,
}

/// Plans one track. It runs in the track's reactor, whose `dash_track` span
/// names the track in its logs.
pub(crate) struct DashPlanner {
    track: MediaTrack,
    selection: Arc<DashSelectionConfig>,
    clock: Arc<dyn Clock>,
    identity: SegmentIdentityPolicy,
    frontier: Arc<ConsumedFrontier>,
    start: StartPosition,
    epochs: VecDeque<Epoch>,
    next_ordinal: u64,
    epoch_serial: u64,
    /// Manifest generation of the last snapshot planned.
    manifest_generation: Option<u64>,
    /// Configured tracks this session does not record, reported once each
    /// when a Period offers them.
    absent_tracks: Vec<MediaTrack>,
    /// Whether a Period lacking this track was reported.
    reported_absence: bool,
    /// The [`TimingConflict`]s reported so far, as a bit set.
    reported_conflicts: u8,
    /// Timing trust of the Periods that may still be planned or still have
    /// epochs; pruned as planning moves past them.
    periods: VecDeque<PeriodTrust>,
    /// Withdraws every anchor issued under the current
    /// `availabilityStartTime`; replaced when it moves.
    presentation_withdrawal: AnchorWithdrawal,
    /// `availabilityStartTime` and Periods of the last dynamic MPD planned.
    last_anchor: Option<(Option<DateTime<Utc>>, Vec<PeriodKey>)>,
    /// How far behind the live edge recording starts; also where a new
    /// epoch starts when nothing else can place it.
    live_start: LiveStartPolicy,
}

pub(crate) struct PlannerParams {
    pub track: MediaTrack,
    pub selection: Arc<DashSelectionConfig>,
    pub clock: Arc<dyn Clock>,
    pub identity: SegmentIdentityPolicy,
    pub frontier: Arc<ConsumedFrontier>,
    pub start: StartPosition,
    pub live_start: LiveStartPolicy,
    /// Tracks to report if a later Period offers them (leader only).
    pub absent_tracks: Vec<MediaTrack>,
}

impl DashPlanner {
    pub(crate) fn new(params: PlannerParams) -> Self {
        Self {
            track: params.track,
            selection: params.selection,
            clock: params.clock,
            identity: params.identity,
            frontier: params.frontier,
            start: params.start,
            epochs: VecDeque::new(),
            next_ordinal: 0,
            epoch_serial: 0,
            manifest_generation: None,
            absent_tracks: params.absent_tracks,
            reported_absence: false,
            reported_conflicts: 0,
            periods: VecDeque::new(),
            presentation_withdrawal: AnchorWithdrawal::default(),
            last_anchor: None,
            live_start: params.live_start,
        }
    }

    /// Whether `conflict` is seen for the first time, marking it reported.
    fn first_conflict(&mut self, conflict: TimingConflict) -> bool {
        let bit = conflict as u8;
        let first = self.reported_conflicts & bit == 0;
        self.reported_conflicts |= bit;
        first
    }

    fn trust_mut(&mut self, period: &PeriodKey) -> &mut PeriodTrust {
        let index = match self
            .periods
            .iter()
            .position(|trust| trust.period.same(period))
        {
            Some(index) => index,
            None => {
                self.periods.push_back(PeriodTrust {
                    period: period.clone(),
                    contradicted: false,
                    withdrawal: AnchorWithdrawal::default(),
                });
                self.periods.len() - 1
            }
        };
        &mut self.periods[index]
    }

    /// Records that MPD updates contradicted `period`'s timing, which
    /// withdraws the wall-clock anchor of all its epochs.
    fn record_contradiction(&mut self, period: &PeriodKey) {
        let trust = self.trust_mut(period);
        trust.contradicted = true;
        trust.withdrawal.withdraw();
    }

    /// Whether MPD updates contradicted `period`'s timing in this session.
    /// Only then may its `SegmentTimeline` overrule the clock (see
    /// [`timeline_contradicts_clock`]), so a single MPD, however its
    /// timeline relates to the clock, never does; and only then are its
    /// media times not compared across epochs.
    fn contradicted(&self, period: &PeriodKey) -> bool {
        self.periods
            .iter()
            .any(|trust| trust.contradicted && trust.period.same(period))
    }

    /// Forgets the trust of Periods planning has moved past and that no
    /// retained epoch belongs to.
    fn prune_periods(&mut self, lower: &PeriodKey) {
        let epochs = &self.epochs;
        self.periods.retain(|trust| {
            trust.period.at_or_after(lower)
                || epochs.iter().any(|epoch| epoch.period.same(&trust.period))
        });
    }

    /// Notes a dynamic MPD's `availabilityStartTime` moving: since the
    /// previous MPD planned, for the Periods both describe, and since each
    /// epoch opened. Runs before anything is reissued or planned. A move
    /// between MPDs withdraws every anchor issued under the old value; an
    /// epoch opened under an older value withdraws its own Period's.
    fn note_moved_anchor(&mut self, manifest: &Manifest) {
        if manifest.presentation_type != PresentationType::Dynamic {
            return;
        }
        let ast = manifest.availability_start_time;
        let periods: Vec<PeriodKey> = manifest.periods.iter().map(PeriodKey::of).collect();
        let mut moved: Vec<(PeriodKey, Option<DateTime<Utc>>)> = Vec::new();
        let transition = self
            .last_anchor
            .as_ref()
            .is_some_and(|(previous_ast, _)| *previous_ast != ast);
        if let Some((previous_ast, previous_periods)) = &self.last_anchor
            && transition
        {
            moved.extend(
                periods
                    .iter()
                    .filter(|period| previous_periods.iter().any(|key| key.same(period)))
                    .map(|period| (period.clone(), *previous_ast)),
            );
        }
        self.last_anchor = Some((ast, periods));
        moved.extend(
            self.epochs
                .iter()
                .filter(|epoch| anchor_moved(manifest, epoch))
                .map(|epoch| (epoch.period.clone(), epoch.availability_start)),
        );
        let Some((_, from)) = moved.first().cloned() else {
            return;
        };
        // Every anchor issued under the old `availabilityStartTime`, in any
        // Period and epoch, retired or not. Only at the transition itself:
        // an epoch still queued from before it keeps differing from later
        // MPDs, and must not withdraw anchors issued under the new value;
        // its own Period's withdrawal covers it.
        if transition {
            self.presentation_withdrawal.withdraw();
            self.presentation_withdrawal = AnchorWithdrawal::default();
        }
        for (period, _) in &moved {
            self.record_contradiction(period);
        }
        let from = from.map(|time| time.to_rfc3339());
        let to = manifest
            .availability_start_time
            .map(|time| time.to_rfc3339());
        if self.first_conflict(TimingConflict::AnchorMoved) {
            warn!(
                from = ?from,
                to = ?to,
                "an MPD update moved availabilityStartTime; output is no longer anchored to wall-clock time"
            );
        } else {
            debug!(from = ?from, to = ?to, "availabilityStartTime moved");
        }
    }

    /// Notes every epoch of a `$Number$` timeline whose own Representation
    /// `manifest` describes at contradicting media times, compared by
    /// number, before anything is reissued or planned, so the clock may be
    /// overruled for this very MPD.
    fn note_rewritten_times(&mut self, manifest: &Manifest) {
        for index in 0..self.epochs.len() {
            let epoch = &self.epochs[index];
            if !(epoch.fingerprint.timeline
                && epoch.fingerprint.addressing == AddressingKind::Number)
            {
                continue;
            }
            let Some(period) = manifest
                .periods
                .iter()
                .find(|period| PeriodKey::of(period).same(&epoch.period))
            else {
                continue;
            };
            let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id).filter(
                    |representation| {
                        representation.uses_segment_timeline()
                            && representation.addressing_kind() == AddressingKind::Number
                    },
                )
            else {
                continue;
            };
            let Some(rewritten) = rewritten_time(representation, epoch) else {
                continue;
            };
            let period = epoch.period.clone();
            let id = Arc::clone(&epoch.representation_id);
            self.report_rewritten(&id, &rewritten);
            self.record_contradiction(&period);
        }
    }

    fn plan_snapshot(
        &mut self,
        snapshot: &MpdSnapshot,
        outcome: &mut PlanOutcome,
    ) -> Result<(), ManifestError> {
        let manifest = snapshot.manifest.as_ref();
        let now = snapshot.clock_offset.apply(self.clock.now());
        // Past `availabilityEndTime` nothing is available any more; evaluating
        // at that instant plans the segments that became available just
        // before it, then the stream ends.
        let ended = manifest.availability_end_time.is_some_and(|end| now >= end);
        let now = match manifest.availability_end_time {
            Some(end) if ended => end,
            _ => now,
        };

        let frontier = self.frontier.get().min(self.next_ordinal);
        self.forget_consumed(frontier);
        self.note_moved_anchor(manifest);
        self.note_rewritten_times(manifest);
        if self.manifest_generation != Some(snapshot.manifest_generation) {
            self.manifest_generation = Some(snapshot.manifest_generation);
            self.reissue(snapshot, now, outcome)?;
        }

        let first_new = self.next_ordinal;
        let mut budget = (frontier + PLANNING_LOOKAHEAD)
            .saturating_sub(self.next_ordinal)
            .min(PLANNING_LOOKAHEAD);
        let mut blocked = false;
        let mut window_len: u64 = 0;
        let mut target_duration = 0.0_f64;
        let lower = match self.epochs.back() {
            Some(epoch) => epoch.period.clone(),
            None => self.start.period.clone(),
        };
        self.prune_periods(&lower);
        // Whether every segment of the Periods iterated so far is planned,
        // including one that overlaps a Period's end and only becomes
        // available after the next Period has begun.
        let mut earlier_complete = true;
        for period in &manifest.periods {
            let key = PeriodKey::of(period);
            if !key.at_or_after(&lower) {
                continue;
            }
            let representation = match self.representation_for(period, &key) {
                Ok(Some(representation)) => representation,
                Ok(None) => {
                    // Lets the muxer record the other track through this
                    // Period, and keeps this track's stall detection quiet.
                    // Only once everything before it is planned: the notice
                    // tells the muxer that no earlier media of this track
                    // remains beyond `after_sequence`.
                    if earlier_complete && has_started(manifest, period, now) {
                        outcome.notices.push(EngineNotice::TrackAbsent {
                            section_id: key.id.clone(),
                            section_start: key.start,
                            after_sequence: self.next_ordinal,
                        });
                    }
                    continue;
                }
                // Work planned before this Period is delivered first; the
                // error ends the stream once nothing earlier is outstanding.
                Err(error) if self.next_ordinal > frontier => {
                    debug!(%error, "planning paused before an unusable Period");
                    blocked = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let overrule = self.contradicted(&key);
            let Availability { window, at, lead } = available(representation, now, overrule)?;
            window_len = window_len.max(window.len());
            let scope = PeriodScope {
                snapshot,
                period,
                key: &key,
                representation,
                at,
                lead,
            };
            match self.plan_period(scope, &window, &mut budget, &mut target_duration, outcome)? {
                PeriodPlan::Complete => {}
                PeriodPlan::Blocked => {
                    blocked = true;
                    break;
                }
            }
            let nothing_to_plan =
                window.is_empty() && manifest.presentation_type == PresentationType::Static;
            earlier_complete &= self.period_fully_planned(period, &key, nothing_to_plan);
        }

        let planned = self.next_ordinal - first_new;
        if planned > 0 {
            outcome.notices.push(EngineNotice::ManifestRefreshed {
                sequence_base: first_new,
                target_duration,
            });
        }
        let complete = !blocked;
        let finished = manifest.presentation_type == PresentationType::Static || ended;
        if finished && complete {
            outcome.terminal.get_or_insert(TerminalCause::End);
        }
        // Consumed segments are never issued again, so their finished
        // records may go. Unfinished ones below the frontier (skipped by the
        // assembler while still retrying) are the backlog: bounded by the
        // availability window for live streams, kept for static ones.
        let backlog = match manifest.presentation_type {
            PresentationType::Static => usize::MAX,
            PresentationType::Dynamic => usize::try_from(window_len.max(1)).unwrap_or(usize::MAX),
        };
        outcome.prune = Some(PruneWindow {
            start: frontier,
            len: backlog,
        });
        Ok(())
    }

    /// Whether no further segment of `period` can be planned: its last
    /// planned segment reaches the Period's end (an open Period never
    /// qualifies), or it has no segments at all (`nothing_to_plan`).
    fn period_fully_planned(
        &self,
        period: &Period,
        key: &PeriodKey,
        nothing_to_plan: bool,
    ) -> bool {
        let Some(duration) = period.duration else {
            return false;
        };
        let end =
            i128::try_from(period.start.saturating_add(duration).as_nanos()).unwrap_or(i128::MAX);
        match self
            .epochs
            .iter()
            .rev()
            .find(|epoch| epoch.period.same(key))
        {
            Some(epoch) => epoch.last_end >= end,
            None => nothing_to_plan,
        }
    }

    /// Drops bookkeeping for consumed segments and for epochs fully behind
    /// the frontier.
    fn forget_consumed(&mut self, frontier: u64) {
        for epoch in &mut self.epochs {
            while epoch
                .planned
                .front()
                .is_some_and(|planned| planned.ordinal < frontier)
            {
                epoch.planned.pop_front();
            }
        }
        while self.epochs.len() > 1
            && self
                .epochs
                .get(1)
                .is_some_and(|next| next.first_ordinal <= frontier)
        {
            self.epochs.pop_front();
        }
    }

    /// Issues every unconsumed planned segment, and its epoch's init, again
    /// with URLs from the new MPD. Ordinals and keys stay the same, so the
    /// store refreshes the fetch URL of queued and retrying work.
    ///
    /// An epoch whose timing this MPD contradicts lost its wall-clock
    /// anchor before ([`Self::note_moved_anchor`],
    /// [`Self::note_rewritten_times`]), so reissued work does not carry it.
    fn reissue(
        &mut self,
        snapshot: &MpdSnapshot,
        now: DateTime<Utc>,
        outcome: &mut PlanOutcome,
    ) -> Result<(), ManifestError> {
        for index in 0..self.epochs.len() {
            let epoch = &self.epochs[index];
            let Some(period) = snapshot
                .manifest
                .periods
                .iter()
                .find(|period| PeriodKey::of(period).same(&epoch.period))
            else {
                continue;
            };
            // Without its index (only the selected Representation's is
            // read) the epoch's segments cannot be found again; like a
            // vanished Representation, its work keeps its URLs.
            let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id)
                    .filter(|representation| representation.pending_segment_index().is_none())
            else {
                continue;
            };
            let key = epoch.period.clone();
            let scope = PeriodScope {
                snapshot,
                period,
                key: &key,
                representation,
                at: now,
                lead: None,
            };
            let Ok((fingerprint, init_url)) = fingerprint(scope) else {
                continue;
            };
            if fingerprint != epoch.fingerprint {
                // The epoch was superseded; its outstanding work keeps the
                // URLs it was planned with.
                continue;
            }
            let overrule = self.contradicted(&key);
            let Availability { window, at, lead } = available(representation, now, overrule)?;
            let scope = PeriodScope { at, lead, ..scope };
            outcome.descriptors.push(init_descriptor(
                epoch.init_key.clone(),
                init_url,
                epoch.first_ordinal,
                Arc::clone(&epoch.init_meta),
            ));
            let locator = epoch.fingerprint.locator();
            // Each pending segment is looked up on its own: the window
            // between two pending locators can be arbitrarily long.
            for planned in &epoch.planned {
                let index = first_index(&window, |segment| {
                    locate(segment, locator) >= planned.locator
                })?;
                let Some(segment) = window.get(index).transpose()? else {
                    continue;
                };
                if locate(&segment, locator) != planned.locator {
                    continue;
                }
                outcome.descriptors.push(media_descriptor(
                    self.track,
                    scope,
                    epoch,
                    &segment,
                    planned.ordinal,
                    planned.key.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Plans the newly available segments of one Period, within the budget
    /// and the epoch window.
    fn plan_period(
        &mut self,
        scope: PeriodScope<'_>,
        window: &SegmentWindow<'_>,
        budget: &mut u64,
        target_duration: &mut f64,
        outcome: &mut PlanOutcome,
    ) -> Result<PeriodPlan, ManifestError> {
        let PeriodScope {
            key,
            representation,
            ..
        } = scope;
        let (fingerprint, init_url) = fingerprint(scope)?;
        let locator = fingerprint.locator();
        let newest = self.epochs.back();
        // The newest planned locator, when this continues the newest epoch.
        let continuing = newest
            .filter(|epoch| {
                epoch.period.same(key)
                    && *epoch.representation_id == *representation.id
                    && epoch.fingerprint == fingerprint
            })
            .map(|epoch| epoch.last_locator);
        let same_period = newest.map(|epoch| epoch.period.same(key));
        let first_index = match continuing {
            Some(last) => {
                let first = first_index(window, |segment| locate(segment, locator) > last)?;
                if locator == Locator::Number
                    && let Some(next) = window.get(first).transpose()?
                    && next.number > last.saturating_add(1)
                {
                    warn!(
                        after = last,
                        next = next.number,
                        "DASH segments left the MPD before they were planned; the recording skips them"
                    );
                }
                first
            }
            None => match same_period {
                Some(true) => self.switch_index(scope, window, &fingerprint)?,
                Some(false) => 0,
                None if self.start.period.same(key) => match self.start.after_nanos {
                    Some(after) => first_index(window, |segment| scope.bounds(segment).1 > after)?,
                    None => 0,
                },
                None => 0,
            },
        };
        let continuing = continuing.is_some();
        self.note_lead(scope);
        if first_index >= window.len() {
            return Ok(PeriodPlan::Complete);
        }
        if *budget == 0 {
            return Ok(PeriodPlan::Blocked);
        }
        if !continuing {
            // Current epoch plus one: an epoch is opened only once the
            // frontier has reached the newest one.
            let frontier = self.frontier.get();
            if self
                .epochs
                .back()
                .is_some_and(|epoch| epoch.first_ordinal > frontier)
            {
                return Ok(PeriodPlan::Blocked);
            }
            let first = window
                .get(first_index)
                .ok_or(ManifestError::overflow("segment window"))??;
            let init = self.open_epoch(scope, fingerprint, init_url, &first);
            outcome.descriptors.push(init);
        }

        let track = self.track;
        let Some(epoch) = self.epochs.back_mut() else {
            return Ok(PeriodPlan::Complete);
        };
        let mut index = first_index;
        while index < window.len() {
            if *budget == 0 {
                return Ok(PeriodPlan::Blocked);
            }
            let segment = window
                .get(index)
                .ok_or(ManifestError::overflow("segment window"))??;
            index += 1;
            let position = locate(&segment, locator);
            epoch.last_locator = position;
            epoch.last_time = segment.time;
            epoch.last_end = scope.bounds(&segment).1;
            let Some(key) = media_key(&self.identity, scope, epoch, &segment) else {
                warn!(
                    number = segment.number,
                    "skipping a DASH segment with an invalid byte range"
                );
                continue;
            };
            let ordinal = self.next_ordinal;
            outcome.descriptors.push(media_descriptor(
                track,
                scope,
                epoch,
                &segment,
                ordinal,
                key.clone(),
            ));
            epoch.planned.push_back(Planned {
                locator: position,
                ordinal,
                key,
            });
            self.next_ordinal = ordinal
                .checked_add(1)
                .ok_or(ManifestError::overflow("segment ordinal"))?;
            *budget -= 1;
            *target_duration = segment.duration_secs();
        }
        Ok(PeriodPlan::Complete)
    }

    /// Reports a timeline followed ahead of the clock (`scope.lead`), which
    /// stops anchoring the continued epoch (`continuing`) to wall-clock
    /// time. Planning is unaffected.
    fn note_lead(&mut self, scope: PeriodScope<'_>) {
        let representation = scope.representation.id.as_str();
        if let Some(lead) = scope.lead {
            let lead_secs = lead.as_secs_f64();
            if self.first_conflict(TimingConflict::AheadOfClock) {
                warn!(
                    representation,
                    lead_secs,
                    "the MPD's SegmentTimeline describes segments far ahead of its availabilityStartTime; following the timeline"
                );
            } else {
                debug!(
                    representation,
                    lead_secs, "SegmentTimeline ahead of the clock"
                );
            }
        }
    }

    fn report_rewritten(&mut self, representation: &str, rewritten: &RewrittenTime) {
        if self.first_conflict(TimingConflict::TimesRewritten) {
            warn!(
                representation,
                number = rewritten.number,
                time = rewritten.time,
                planned_number = rewritten.planned_number,
                planned_time = rewritten.planned_time,
                "an MPD update contradicts the SegmentTimeline media times of earlier ones; following segment numbers"
            );
        } else {
            debug!(
                representation,
                number = rewritten.number,
                time = rewritten.time,
                "SegmentTimeline media times rewritten"
            );
        }
    }

    /// Where a new epoch in the Period of the newest one starts in
    /// `window`.
    ///
    /// While the timing is trusted, after the newest planned presentation
    /// time. Otherwise, within one numbering sequence (the same
    /// Representation as `$Number$` timelines on both sides), after the
    /// newest planned number, whatever changed in its URLs, initialization
    /// or CDN; else like a recording starts
    /// ([`live_start_target`]). A continued number that the switching MPD
    /// describes at another media time was already noted
    /// ([`Self::note_rewritten_times`]), so it makes the timing untrusted
    /// here. Different Representations may number the same interval
    /// differently, so their numbers are never compared.
    fn switch_index(
        &mut self,
        scope: PeriodScope<'_>,
        window: &SegmentWindow<'_>,
        fingerprint: &Fingerprint,
    ) -> Result<u64, ManifestError> {
        let Some(previous) = self.epochs.back() else {
            return Ok(0);
        };
        let numbered = |fingerprint: &Fingerprint| {
            fingerprint.timeline && fingerprint.addressing == AddressingKind::Number
        };
        let last = previous.last_locator;
        let same_sequence = numbered(fingerprint)
            && numbered(&previous.fingerprint)
            && *previous.representation_id == *scope.representation.id;
        let last_end = previous.last_end;
        let trusted = self.timing_trusted(scope, previous);
        if same_sequence && !trusted {
            return first_index(window, |segment| segment.number > last);
        }
        if trusted {
            return first_index(window, |segment| scope.bounds(segment).1 > last_end);
        }
        self.live_start_index(scope, window)
    }

    /// Where a recording would start in `window` (see
    /// [`live_start_target`]).
    fn live_start_index(
        &self,
        scope: PeriodScope<'_>,
        window: &SegmentWindow<'_>,
    ) -> Result<u64, ManifestError> {
        let Some(last) = window.last().transpose()? else {
            return Ok(0);
        };
        let (last_start, last_end) = scope.bounds(&last);
        let elapsed = scope
            .manifest()
            .availability_start_time
            .and_then(|ast| (scope.at - ast).num_nanoseconds())
            .map(i128::from);
        let target = live_start_target(
            self.live_start,
            scope.manifest(),
            (last_start, last_end),
            elapsed,
        );
        first_index(window, |segment| scope.bounds(segment).1 > target)
    }

    /// Whether `epoch`'s media times can still be compared with those of
    /// `scope`'s MPD: nothing has contradicted the timing of its Period.
    fn timing_trusted(&self, scope: PeriodScope<'_>, epoch: &Epoch) -> bool {
        !self.contradicted(scope.key)
            && scope.lead.is_none()
            && !anchor_moved(scope.manifest(), epoch)
    }

    /// The Representation this track records in `period`: the newest
    /// epoch's while it is there, else a fresh selection. Fails with the
    /// typed reason when the Period offers nothing usable for the track
    /// because it was rejected (DRM, WebM, ...).
    ///
    /// Only the selected Representation's Segment Index is read, so an
    /// epoch's Representation that became `SegmentBase` in a new MPD is
    /// replaced by the selection, as if it had vanished.
    fn representation_for<'a>(
        &mut self,
        period: &'a Period,
        key: &PeriodKey,
    ) -> Result<Option<&'a Representation>, ManifestError> {
        if let Some(epoch) = self.epochs.back().filter(|epoch| epoch.period.same(key))
            && let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id)
            && representation.pending_segment_index().is_none()
        {
            return Ok(Some(representation));
        }
        let selected = selection::select(period, self.track, &self.selection);
        // Its index could not be read (see `OnIndexFailure::Skip`). Failing
        // here, not when listing its segments, lets the caller deliver the
        // work planned before this Period first.
        if let Some(error) = selected.and_then(Representation::unread_segment_index_error) {
            return Err(error);
        }
        if selected.is_none() {
            // A Period that offers this track only in rejected form (DRM,
            // WebM, ...) must not silently drop it; the rejection is the
            // reason it cannot continue. A Period without the track at all
            // is a gap in that track.
            if let Some(feature) = rejection_for(period, self.track) {
                return Err(feature.into());
            }
            if !self.reported_absence {
                self.reported_absence = true;
                info!(
                    period = %key.label(),
                    "a Period does not carry this track; it has a gap there"
                );
            }
        }
        Ok(selected)
    }

    fn open_epoch(
        &mut self,
        scope: PeriodScope<'_>,
        fingerprint: Fingerprint,
        init_url: Url,
        first: &Segment,
    ) -> SegmentDescriptor {
        let PeriodScope {
            key,
            period,
            representation,
            ..
        } = scope;
        self.epoch_serial += 1;
        let epoch_scope: Arc<str> = Arc::from(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.track,
            key.label(),
            representation.id,
            self.epoch_serial
        ));
        let init_key = SegmentKey {
            kind: SegmentKind::Init,
            uri: self.identity.canonical_uri(&init_url),
            byte_range: fingerprint.init_range,
            scope: Some(Arc::clone(&epoch_scope)),
        };
        let availability_start = scope.manifest().availability_start_time;
        // Every epoch of a Period shares its withdrawal, so a contradiction
        // seen later also reaches the output of epochs retired by then.
        let trusted = scope.lead.is_none() && !self.contradicted(key);
        let period_withdrawal = self.trust_mut(key).withdrawal.clone();
        let anchor = availability_start.filter(|_| trusted).map(|time| {
            WallClockAnchor::new(time, &self.presentation_withdrawal, &period_withdrawal)
        });
        let init_meta = Arc::new(segment_meta(self.track, scope, first, 0, anchor.clone()));
        let init = init_descriptor(
            init_key.clone(),
            init_url,
            self.next_ordinal,
            Arc::clone(&init_meta),
        );
        match self.epochs.back() {
            Some(previous) => info!(
                from_period = %previous.period.label(),
                from_representation = %previous.representation_id,
                to_period = %key.label(),
                to_representation = %representation.id,
                "DASH track switches epoch"
            ),
            None => info!(
                period = %key.label(),
                representation = %representation.id,
                "DASH track starts"
            ),
        }
        if representation
            .start_with_sap
            .is_some_and(|sap| sap == 0 || sap > 3)
        {
            // Size and duration splits do not wait for a random-access
            // point, so files cut inside such a stream may start undecodable.
            info!(
                representation = %representation.id,
                "Representation does not declare segments starting with a stream access point"
            );
        }
        let selection = Arc::clone(&self.selection);
        self.absent_tracks.retain(|&absent| {
            if selection::select(period, absent, &selection).is_none() {
                return true;
            }
            warn!(
                track = %absent,
                period = %key.label(),
                "a Period offers a track this recording did not start with; it is not recorded"
            );
            false
        });
        self.epochs.push_back(Epoch {
            period: key.clone(),
            representation_id: Arc::from(representation.id.as_str()),
            last_locator: locate(first, fingerprint.locator()),
            last_time: first.time,
            last_end: scope.bounds(first).0,
            availability_start,
            anchor,
            fingerprint,
            scope: epoch_scope,
            init_key,
            init_meta,
            first_ordinal: self.next_ordinal,
            planned: VecDeque::new(),
        });
        init
    }
}

/// Decides where a session starts recording, once, for every track: the
/// first Period of a static presentation; for a dynamic one, the segment at
/// the configured distance behind the live edge, judged by the reference
/// track (video when recorded, else audio).
pub(crate) fn decide_start(
    manifest: &Manifest,
    now: DateTime<Utc>,
    live_start: LiveStartPolicy,
    selection_config: &DashSelectionConfig,
) -> Result<Option<StartPosition>, ManifestError> {
    let reference = |period| reference_representation(period, selection_config);
    if manifest.presentation_type == PresentationType::Static {
        // Always the first Period: one that cannot be recorded is an error
        // (see `rejection_for`), never silently skipped.
        return Ok(manifest.periods.first().map(|period| StartPosition {
            period: PeriodKey::of(period),
            after_nanos: None,
        }));
    }
    let Some(availability_start) = manifest.availability_start_time else {
        return Err(ManifestError::MissingAttribute {
            element: "MPD",
            attribute: "availabilityStartTime",
        });
    };
    let elapsed_at = |at: DateTime<Utc>| {
        (at - availability_start)
            .num_nanoseconds()
            .map(i128::from)
            .ok_or(ManifestError::overflow("live start"))
    };
    let elapsed = elapsed_at(now)?;

    // The newest Period with an available reference segment that starts at
    // or before the target time.
    for (index, period) in manifest.periods.iter().enumerate().rev() {
        let Some(representation) = reference(period) else {
            continue;
        };
        // One MPD cannot show its timing contradicted, so the clock decides.
        let Availability { window, at, .. } = available(representation, now, false)?;
        let Some(last) = window.last() else {
            continue;
        };
        let last = last?;
        let bounds = segment_bounds(&last, representation, period);
        let target = live_start_target(live_start, manifest, bounds, Some(elapsed_at(at)?));
        let period_start = i128::try_from(period.start.as_nanos()).unwrap_or(i128::MAX);
        if target >= period_start || index == 0 {
            return Ok(Some(StartPosition {
                period: PeriodKey::of(period),
                after_nanos: Some(target),
            }));
        }
    }
    // Nothing is available yet: start with the newest Period that has begun,
    // from whatever becomes available first.
    let started = manifest
        .periods
        .iter()
        .rev()
        .find(|period| {
            reference(period).is_some()
                && i128::try_from(period.start.as_nanos()).is_ok_and(|start| start <= elapsed)
        })
        .or_else(|| {
            manifest
                .periods
                .iter()
                .find(|period| reference(period).is_some())
        });
    Ok(started.map(|period| StartPosition {
        period: PeriodKey::of(period),
        after_nanos: None,
    }))
}

/// The presentation time after which recording starts, given the newest
/// available segment's bounds and the time elapsed since
/// `availabilityStartTime` (the newest segment's end when unknown).
fn live_start_target(
    live_start: LiveStartPolicy,
    manifest: &Manifest,
    (last_start, last_end): (i128, i128),
    elapsed: Option<i128>,
) -> i128 {
    match live_start {
        LiveStartPolicy::LiveEdge => last_start,
        LiveStartPolicy::PresentationDelay(delay) => {
            let delay = match delay.or(manifest.suggested_presentation_delay) {
                Some(delay) => i128::try_from(delay.as_nanos()).unwrap_or(i128::MAX),
                None => (last_end - last_start).saturating_mul(3),
            };
            // Never later than the newest segment, which would plan
            // nothing until the clock catches up with the delay.
            elapsed
                .unwrap_or(last_end)
                .saturating_sub(delay)
                .min(last_start)
        }
    }
}

/// Whether `period` has begun at `now` (always, for static presentations).
fn has_started(manifest: &Manifest, period: &Period, now: DateTime<Utc>) -> bool {
    let Some(availability_start) = manifest
        .availability_start_time
        .filter(|_| manifest.presentation_type == PresentationType::Dynamic)
    else {
        return true;
    };
    (now - availability_start)
        .to_std()
        .is_ok_and(|elapsed| elapsed >= period.start)
}

/// The unsupported feature that kept `track` out of `period`, when a
/// rejected Representation was (or may have been) of that track's type.
pub(crate) fn rejection_for(period: &Period, track: MediaTrack) -> Option<UnsupportedFeature> {
    let wanted = match track {
        MediaTrack::Video => RejectedContent::Video,
        MediaTrack::Audio => RejectedContent::Audio,
    };
    period
        .rejected
        .iter()
        .find(|rejected| rejected.content == wanted || rejected.content == RejectedContent::Unknown)
        .map(|rejected| rejected.feature.clone())
}

/// The Representation that decides timing for a session: video when
/// recorded, else audio.
fn reference_representation<'a>(
    period: &'a Period,
    selection_config: &DashSelectionConfig,
) -> Option<&'a Representation> {
    let video = selection_config
        .include_video
        .then(|| selection::select(period, MediaTrack::Video, selection_config))
        .flatten();
    video.or_else(|| {
        selection_config
            .include_audio
            .then(|| selection::select(period, MediaTrack::Audio, selection_config))
            .flatten()
    })
}

impl SegmentPlanner for DashPlanner {
    type Snapshot = MpdSnapshot;

    fn plan(&mut self, snapshot: &MpdSnapshot) -> PlanOutcome {
        let mut outcome = PlanOutcome {
            generation: snapshot.generation,
            terminal: snapshot.terminal.clone(),
            ..PlanOutcome::default()
        };
        if matches!(snapshot.terminal, Some(TerminalCause::Failed(_))) {
            return outcome;
        }
        if let Err(error) = self.plan_snapshot(snapshot, &mut outcome) {
            warn!(%error, "DASH planning failed");
            return PlanOutcome {
                generation: snapshot.generation,
                terminal: Some(TerminalCause::Failed(SharedError::new(error.into()))),
                ..PlanOutcome::default()
            };
        }
        outcome
    }

    fn failure_error(&self, failure: SourceFailure) -> DownloadError {
        let reason = match failure {
            SourceFailure::Reset(kind) => format!("{} timeline reset ({kind:?})", self.track),
            SourceFailure::Lost => "MPD watcher stopped without a terminal cause".to_owned(),
        };
        DownloadError::Manifest { reason }
    }
}

/// How an MPD update contradicts the media times of a `$Number$`
/// timeline's newest planned segment.
struct RewrittenTime {
    planned_number: u64,
    planned_time: u64,
    number: u64,
    time: u64,
}

/// Whether `representation`'s timeline lists the newest segment `epoch`
/// planned at another media time, or lists the first segment after it as
/// starting no later than that time. Only meaningful within one `$Number$`
/// numbering sequence: the caller passes the epoch's own Representation.
/// The listing is read whatever the clock says, so a description placing
/// everything far ahead is compared too. Within one MPD the timeline is
/// validated to increase, so these are the only places an update can
/// contradict the media times planned from earlier ones.
fn rewritten_time(representation: &Representation, epoch: &Epoch) -> Option<RewrittenTime> {
    let (planned_number, planned_time) = (epoch.last_locator, epoch.last_time);
    let (number, time) = representation.listed_from(planned_number)?;
    // Presentation times `(t − PTO) / timescale`, compared exactly by
    // cross-multiplying: the epochs may use different timescales.
    let offset = |time: u64, pto: u64| i128::from(time) - i128::from(pto);
    let planned = offset(planned_time, epoch.fingerprint.presentation_time_offset)
        .checked_mul(i128::from(representation.timescale().max(1)))?;
    let listed = offset(time, representation.presentation_time_offset())
        .checked_mul(i128::from(epoch.fingerprint.timescale.max(1)))?;
    let contradicts = if number == planned_number {
        listed != planned
    } else {
        listed <= planned
    };
    contradicts.then_some(RewrittenTime {
        planned_number,
        planned_time,
        number,
        time,
    })
}

/// The first window index whose segment satisfies `pred`, which must be
/// false for a prefix of the window and true for the rest. `window.len()`
/// when none does. Binary search keeps long windows cheap.
fn first_index(
    window: &SegmentWindow<'_>,
    mut pred: impl FnMut(&Segment) -> bool,
) -> Result<u64, ManifestError> {
    let (mut low, mut high) = (0, window.len());
    while low < high {
        let middle = low + (high - low) / 2;
        let segment = window
            .get(middle)
            .ok_or(ManifestError::overflow("segment window"))??;
        if pred(&segment) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

/// Start and end of a segment on the MPD timeline, in nanoseconds: `P +
/// (t − PTO) / T` (DASH-IF timing model §9.1).
fn segment_bounds(
    segment: &Segment,
    representation: &Representation,
    period: &Period,
) -> (i128, i128) {
    let timescale = i128::from(representation.timescale().max(1));
    let period_start = i128::try_from(period.start.as_nanos()).unwrap_or(i128::MAX);
    let offset = i128::from(segment.time) - i128::from(representation.presentation_time_offset());
    let end_offset = offset.saturating_add(i128::from(segment.duration));
    // Each bound is floored from its exact tick position, never derived from
    // another rounded value: a Period end is a whole nanosecond, so a floored
    // end reaches it exactly when the true end does.
    let to_nanos = |ticks: i128| ticks.saturating_mul(NANOS_PER_SEC).div_euclid(timescale);
    let start = period_start.saturating_add(to_nanos(offset));
    let end = period_start.saturating_add(to_nanos(end_offset));
    (start, end)
}

fn byte_range_key(range: ByteRange) -> Option<ByteRangeKey> {
    let length = range.last.checked_sub(range.first)?.checked_add(1)?;
    Some(ByteRangeKey {
        length,
        offset: range.first,
    })
}

/// The Representation's epoch fingerprint and initialization URL (with
/// the MPD's query parameters inherited).
fn fingerprint(scope: PeriodScope<'_>) -> Result<(Fingerprint, Url), ManifestError> {
    let representation = scope.representation;
    let Some(initialization) = &representation.initialization else {
        return Err(ManifestError::inconsistent(
            "selected Representation has no initialization segment",
        ));
    };
    let init_range = match initialization.byte_range {
        Some(range) => Some(byte_range_key(range).ok_or_else(|| {
            ManifestError::inconsistent("initialization byte range ends before it starts")
        })?),
        None => None,
    };
    let mut location = initialization.url.clone();
    location.set_query(None);
    location.set_fragment(None);
    let mut url = initialization.url.clone();
    inherit_query_params(&mut url, &scope.snapshot.parent_params);
    Ok((
        Fingerprint {
            init_location: location.into(),
            init_range,
            timescale: representation.timescale(),
            presentation_time_offset: representation.presentation_time_offset(),
            timeline: representation.uses_segment_timeline(),
            addressing: representation.addressing_kind(),
        },
        url,
    ))
}

fn segment_meta(
    track: MediaTrack,
    scope: PeriodScope<'_>,
    segment: &Segment,
    duration: u64,
    availability_start_time: Option<WallClockAnchor>,
) -> DashSegmentMeta {
    let PeriodScope {
        period,
        representation,
        ..
    } = scope;
    DashSegmentMeta {
        track,
        period_id: period.id.as_deref().map(Arc::from),
        period_start: period.start,
        representation_id: Arc::from(representation.id.as_str()),
        number: segment.number,
        time: segment.time,
        duration,
        timescale: segment.timescale,
        presentation_time_offset: representation.presentation_time_offset(),
        availability_start_time,
    }
}

fn init_descriptor(
    key: SegmentKey,
    url: Url,
    ordinal: u64,
    meta: Arc<DashSegmentMeta>,
) -> SegmentDescriptor {
    SegmentDescriptor {
        key,
        msn: ordinal,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(url),
        discontinuity: false,
        encryption: None,
        init_key: None,
        meta: SegmentMeta::Dash(meta),
    }
}

fn segment_url(scope: PeriodScope<'_>, segment: &Segment) -> Url {
    let mut url: Url = segment.url.clone();
    inherit_query_params(&mut url, &scope.snapshot.parent_params);
    url
}

/// The identity of a newly planned segment; `None` for an invalid byte
/// range. Reissued segments keep the key they were planned with.
fn media_key(
    identity: &SegmentIdentityPolicy,
    scope: PeriodScope<'_>,
    epoch: &Epoch,
    segment: &Segment,
) -> Option<SegmentKey> {
    let byte_range = match segment.byte_range {
        Some(range) => Some(byte_range_key(range)?),
        None => None,
    };
    Some(SegmentKey {
        kind: SegmentKind::Media,
        uri: identity.canonical_uri(&segment_url(scope, segment)),
        byte_range,
        scope: Some(Arc::clone(&epoch.scope)),
    })
}

fn media_descriptor(
    track: MediaTrack,
    scope: PeriodScope<'_>,
    epoch: &Epoch,
    segment: &Segment,
    ordinal: u64,
    key: SegmentKey,
) -> SegmentDescriptor {
    let meta = segment_meta(
        track,
        scope,
        segment,
        segment.duration,
        epoch.anchor.clone(),
    );
    SegmentDescriptor {
        key,
        msn: ordinal,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(segment_url(scope, segment)),
        discontinuity: false,
        encryption: None,
        init_key: Some(epoch.init_key.clone()),
        meta: SegmentMeta::Dash(Arc::new(meta)),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::dash::manifest::ManualClock;

    const MPD_URL: &str = "https://cdn.example.com/live/manifest.mpd?token=abc";

    fn ast() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
    }

    /// A planner driven like the engine drives it: the start is decided
    /// from the first MPD, the manifest generation advances only when the
    /// MPD changes, and consumption is simulated through the frontier.
    struct Fixture {
        track: MediaTrack,
        live_start: LiveStartPolicy,
        clock: Arc<ManualClock>,
        frontier: Arc<ConsumedFrontier>,
        planner: Option<DashPlanner>,
        generation: u64,
        manifest_generation: u64,
        last_mpd: Option<String>,
    }

    impl Fixture {
        fn new(track: MediaTrack, live_start: LiveStartPolicy, elapsed_secs: i64) -> Self {
            Self {
                track,
                live_start,
                clock: Arc::new(ManualClock::new(
                    ast() + chrono::TimeDelta::seconds(elapsed_secs),
                )),
                frontier: Arc::new(ConsumedFrontier::default()),
                planner: None,
                generation: 0,
                manifest_generation: 0,
                last_mpd: None,
            }
        }

        fn snapshot(&mut self, mpd: &str, edit: impl FnOnce(&mut Manifest)) -> MpdSnapshot {
            let url = Url::parse(MPD_URL).unwrap();
            let mut manifest = Manifest::parse(mpd.as_bytes(), &url).expect("valid MPD");
            edit(&mut manifest);
            self.generation += 1;
            if self.last_mpd.as_deref() != Some(mpd) {
                self.manifest_generation += 1;
                self.last_mpd = Some(mpd.to_owned());
            }
            MpdSnapshot {
                generation: self.generation,
                manifest_generation: self.manifest_generation,
                manifest: Arc::new(manifest),
                document_url: Arc::new(url.clone()),
                parent_params: crate::segment_engine::descriptor::query_params(url.query()).into(),
                fetched_at: self.clock.now(),
                clock_offset: crate::dash::manifest::ClockOffset::ZERO,
                terminal: None,
            }
        }

        fn plan(&mut self, mpd: &str) -> PlanOutcome {
            self.plan_edited(mpd, |_| {})
        }

        /// Plans `mpd` after `edit` changed its resolved form, as the
        /// watcher does when it applies Segment Indexes.
        fn plan_edited(&mut self, mpd: &str, edit: impl FnOnce(&mut Manifest)) -> PlanOutcome {
            let snapshot = self.snapshot(mpd, edit);
            let planner = self.planner.get_or_insert_with(|| {
                let start = decide_start(
                    &snapshot.manifest,
                    self.clock.now(),
                    self.live_start,
                    &DashSelectionConfig::default(),
                )
                .expect("start decision")
                .expect("a starting Period");
                DashPlanner::new(PlannerParams {
                    track: self.track,
                    selection: Arc::new(DashSelectionConfig::default()),
                    clock: Arc::clone(&self.clock) as Arc<dyn Clock>,
                    identity: SegmentIdentityPolicy::default(),
                    frontier: Arc::clone(&self.frontier),
                    start,
                    live_start: self.live_start,
                    absent_tracks: Vec::new(),
                })
            });
            planner.plan(&snapshot)
        }

        /// The assembler emitted everything planned so far.
        fn consume_all(&self) {
            if let Some(planner) = &self.planner {
                self.frontier.advance_to(planner.next_ordinal);
            }
        }

        fn advance(&self, secs: u64) {
            self.clock.advance(std::time::Duration::from_secs(secs));
        }
    }

    fn media(outcome: &PlanOutcome) -> Vec<(u64, &DashSegmentMeta, &str)> {
        outcome
            .descriptors
            .iter()
            .filter(|descriptor| descriptor.key.kind == SegmentKind::Media)
            .map(|descriptor| match &descriptor.meta {
                SegmentMeta::Dash(meta) => (
                    descriptor.msn,
                    meta.as_ref(),
                    descriptor.parsed_url.as_str(),
                ),
                SegmentMeta::Hls(_) => panic!("DASH planner produced HLS metadata"),
            })
            .collect()
    }

    /// The wall-clock anchor a segment's output would carry now.
    fn wall_clock(meta: &DashSegmentMeta) -> Option<DateTime<Utc>> {
        meta.availability_start_time
            .as_ref()
            .and_then(WallClockAnchor::get)
    }

    fn init(outcome: &PlanOutcome) -> &SegmentDescriptor {
        outcome
            .descriptors
            .iter()
            .find(|descriptor| descriptor.key.kind == SegmentKind::Init)
            .expect("init descriptor")
    }

    fn number_mpd(extra: &str) -> String {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S" {extra}>
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="2000" timescale="1000" startNumber="1"/>
      <Representation id="v1" bandwidth="1000000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="en">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2000" timescale="1000" startNumber="1"/>
      <Representation id="a1" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
        )
    }

    #[test]
    fn number_template_starts_behind_the_edge_and_plans_new_segments_once() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 20);
        let mpd = number_mpd("");
        let first = fixture.plan(&mpd);
        // 3 × 2 s behind t=20 s → the segment covering 14 s, number 8.
        let numbers: Vec<(u64, u64)> = media(&first)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.number))
            .collect();
        assert_eq!(numbers, [(0, 8), (1, 9), (2, 10)]);
        assert_eq!(
            init(&first).key.scope,
            media(&first)
                .first()
                .map(|_| first.descriptors[0].key.scope.clone())
                .unwrap()
        );
        assert!(first.terminal.is_none());
        assert_eq!(first.prune.map(|prune| prune.start), Some(0));
        assert!(matches!(
            first.notices.as_slice(),
            [EngineNotice::ManifestRefreshed {
                sequence_base: 0,
                ..
            }]
        ));

        let again = fixture.plan(&mpd);
        assert!(media(&again).is_empty(), "nothing is planned twice");
        assert!(again.notices.is_empty());

        fixture.consume_all();
        fixture.advance(4);
        let next = fixture.plan(&mpd);
        let numbers: Vec<(u64, u64)> = media(&next)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.number))
            .collect();
        assert_eq!(numbers, [(3, 11), (4, 12)]);
    }

    #[test]
    fn segment_urls_inherit_the_mpd_query_and_carry_epoch_identity() {
        let mut fixture = Fixture::new(MediaTrack::Audio, LiveStartPolicy::LiveEdge, 20);
        let outcome = fixture.plan(&number_mpd(""));
        let planned = media(&outcome);
        assert_eq!(planned.len(), 1, "the live edge is the newest segment");
        assert_eq!(
            planned[0].2,
            "https://cdn.example.com/live/a/10.m4s?token=abc"
        );
        assert_eq!(
            init(&outcome).parsed_url.as_str(),
            "https://cdn.example.com/live/a/init.mp4?token=abc"
        );
        let media_descriptor = outcome
            .descriptors
            .iter()
            .find(|descriptor| descriptor.key.kind == SegmentKind::Media)
            .unwrap();
        assert_eq!(
            media_descriptor.init_key.as_ref(),
            Some(&init(&outcome).key)
        );
        assert!(media_descriptor.key.scope.is_some());
        assert_eq!(
            media_descriptor.meta.container(),
            Some(crate::segment_engine::container::ContainerKind::Fmp4)
        );
    }

    #[test]
    fn timeline_generations_keep_consecutive_ordinals_when_entries_expire() {
        let timeline = |entries: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v1" bandwidth="1000000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        let first = fixture.plan(&timeline(r#"<S t="0" d="2000" r="4"/>"#));
        let times: Vec<(u64, u64)> = media(&first)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.time))
            .collect();
        assert_eq!(times, [(0, 8000)]);

        fixture.consume_all();
        fixture.advance(4);
        // The first three entries expired; numbers shift, times do not.
        let next = fixture.plan(&timeline(r#"<S t="6000" d="2000" r="3"/>"#));
        let times: Vec<(u64, u64)> = media(&next)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.time))
            .collect();
        assert_eq!(times, [(1, 10000), (2, 12000)]);
        assert!(media(&next).iter().all(|(_, _, url)| url.contains("/v/1")));
    }

    #[test]
    fn period_change_opens_a_new_epoch_with_its_own_init() {
        let two_periods = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p0/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p1/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(
            MediaTrack::Video,
            LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(4))),
            8,
        );
        let first = fixture.plan(two_periods);
        let first_init = init(&first).key.clone();
        assert_eq!(
            media(&first)
                .iter()
                .map(|(msn, _, _)| *msn)
                .collect::<Vec<_>>(),
            [0, 1]
        );

        fixture.consume_all();
        fixture.advance(6);
        let next = fixture.plan(two_periods);
        let planned: Vec<(u64, Option<&str>)> = media(&next)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.period_id.as_deref()))
            .collect();
        assert_eq!(planned, [(2, Some("p0")), (3, Some("p1")), (4, Some("p1"))]);
        let p1_media = next
            .descriptors
            .iter()
            .find(|descriptor| descriptor.msn == 3 && descriptor.key.kind == SegmentKind::Media)
            .unwrap();
        let second_init = p1_media.init_key.clone().unwrap();
        assert!(
            next.descriptors
                .iter()
                .any(|descriptor| descriptor.key == second_init),
            "the new epoch's init is planned with its first media"
        );
        assert_eq!(second_init.uri, first_init.uri, "same URL in both Periods");
        assert_ne!(second_init, first_init, "but a distinct resource");
    }

    #[test]
    fn a_vanished_representation_is_replaced_by_a_new_epoch() {
        let with = |id: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4" duration="2" timescale="1"/>
      <Representation id="{id}" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        let first = fixture.plan(&with("hd"));
        assert_eq!(media(&first)[0].1.number, 5);
        fixture.consume_all();
        fixture.advance(2);
        let next = fixture.plan(&with("sd"));
        let planned = media(&next);
        assert_eq!(planned.len(), 1);
        assert_eq!((planned[0].0, planned[0].1.number), (1, 6));
        assert_eq!(&*planned[0].1.representation_id, "sd");
        assert_ne!(init(&next).key, init(&first).key);
    }

    #[test]
    fn a_representation_whose_index_was_not_read_is_replaced() {
        let live = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="low" bandwidth="100">
        <SegmentTemplate media="low/$Time$.m4s" initialization="low.mp4" timescale="1">
          <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        // The final MPD moves both Representations to `SegmentBase`; only
        // the selected one's index is read. `low` keeps its initialization,
        // timescale and timeline, so its epoch's fingerprint is unchanged.
        let finished = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="low" bandwidth="100"><BaseURL>low.mp4</BaseURL><SegmentBase indexRange="100-199"><Initialization/></SegmentBase></Representation>
      <Representation id="high" bandwidth="900"><BaseURL>high.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        assert!(!media(&fixture.plan(live)).is_empty());
        let next = fixture.plan_edited(finished, |manifest| {
            let subsegments: Vec<_> = (0..7)
                .map(|n| crate::dash::manifest::Subsegment {
                    range: ByteRange {
                        first: 200 + n * 10,
                        last: 209 + n * 10,
                    },
                    duration: 2,
                })
                .collect();
            let index =
                crate::dash::manifest::SubsegmentIndex::new(1, 0, 100, &subsegments).unwrap();
            let high = &mut manifest.periods[0].adaptation_sets[0].representations[1];
            high.apply_segment_index(&index).unwrap();
        });
        // Everything is planned, so the static presentation ends; it does
        // not fail on the unread `low`.
        assert!(
            matches!(next.terminal, Some(TerminalCause::End)),
            "{:?}",
            next.terminal
        );
        let planned = media(&next);
        assert!(!planned.is_empty());
        assert!(
            planned
                .iter()
                .all(|(_, meta, _)| &*meta.representation_id == "high")
        );
    }

    #[test]
    fn an_unread_index_the_planner_does_not_need_is_ignored() {
        let template = r#"<SegmentTemplate media="low/$Number$.m4s" initialization="low/init.mp4" duration="2" timescale="1"/>"#;
        let live = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
</MPD>"#
        );
        // The final MPD adds `high`, whose index could not be read; the
        // recording keeps `low` to its end.
        let finished = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100">{template}</Representation>
    <Representation id="high" bandwidth="900"><BaseURL>high.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
  </AdaptationSet></Period>
</MPD>"#
        );
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        assert!(!media(&fixture.plan(&live)).is_empty());
        fixture.consume_all();
        let next = fixture.plan(&finished);
        assert!(
            matches!(next.terminal, Some(TerminalCause::End)),
            "{:?}",
            next.terminal
        );
        let planned = media(&next);
        assert_eq!(
            planned
                .iter()
                .map(|(_, meta, _)| (&*meta.representation_id, meta.number))
                .collect::<Vec<_>>(),
            [("low", 6), ("low", 7)]
        );
    }

    #[test]
    fn work_planned_before_a_switch_to_indexed_addressing_keeps_its_urls() {
        let live = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100">
      <SegmentTemplate media="low/$Time$.m4s" initialization="low.mp4" timescale="1">
        <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
      </SegmentTemplate>
    </Representation>
  </AdaptationSet></Period>
</MPD>"#;
        // Same Representation, initialization, timescale and timeline, but
        // now byte ranges of `low.mp4`.
        let finished = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100"><BaseURL>low.mp4</BaseURL><SegmentBase indexRange="100-199"><Initialization/></SegmentBase></Representation>
  </AdaptationSet></Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        // Planned, not yet downloaded: reissued on the next MPD.
        assert!(!media(&fixture.plan(live)).is_empty());
        let next = fixture.plan_edited(finished, |manifest| {
            let subsegments: Vec<_> = (0..7)
                .map(|n| crate::dash::manifest::Subsegment {
                    range: ByteRange {
                        first: 200 + n * 10,
                        last: 209 + n * 10,
                    },
                    duration: 2,
                })
                .collect();
            let index =
                crate::dash::manifest::SubsegmentIndex::new(1, 0, 100, &subsegments).unwrap();
            let low = &mut manifest.periods[0].adaptation_sets[0].representations[0];
            low.apply_segment_index(&index).unwrap();
        });
        let indexed: Vec<_> = next
            .descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.key.kind == SegmentKind::Media
                    && descriptor.parsed_url.path().ends_with("/low.mp4")
            })
            .collect();
        assert!(!indexed.is_empty());
        // Never the whole resource under a key planned for a template URL.
        assert!(
            indexed
                .iter()
                .all(|descriptor| descriptor.key.byte_range.is_some())
        );
    }

    #[test]
    fn an_unread_index_in_a_later_period_waits_for_earlier_work() {
        let template = r#"<SegmentTemplate media="low/$Number$.m4s" initialization="low/init.mp4" duration="2" timescale="1"/>"#;
        let live = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
</MPD>"#
        );
        // `p1`'s only Representation has an unread index.
        let finished = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT20S">
  <Period id="p0" start="PT0S" duration="PT10S"><AdaptationSet mimeType="video/mp4">
    <Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
  <Period id="p1" start="PT10S"><AdaptationSet mimeType="video/mp4">
    <Representation id="v" bandwidth="100"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
  </AdaptationSet></Period>
</MPD>"#
        );
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 6);
        assert!(!media(&fixture.plan(&live)).is_empty());
        fixture.consume_all();
        let next = fixture.plan(&finished);
        assert_eq!(next.terminal, None);
        assert_eq!(
            media(&next)
                .iter()
                .map(|(_, meta, _)| meta.number)
                .collect::<Vec<_>>(),
            [4, 5]
        );
        fixture.consume_all();
        let last = fixture.plan(&finished);
        assert!(
            matches!(&last.terminal, Some(TerminalCause::Failed(error)) if error.to_string().contains("has not been read")),
            "{:?}",
            last.terminal
        );
        // When the read failed, the stream ends with that failure.
        let failed = fixture.plan_edited(&finished, |manifest| {
            manifest.periods[1].adaptation_sets[0].representations[0].fail_segment_index(
                SharedError::new(DownloadError::InvalidContent {
                    protocol: "dash",
                    reason: "corrupt sidx".to_owned(),
                }),
            );
        });
        let Some(TerminalCause::Failed(error)) = failed.terminal else {
            panic!("expected a failure, got {:?}", failed.terminal);
        };
        let error = error.to_error();
        assert!(
            matches!(&error, DownloadError::InvalidContent { reason, .. } if reason == "corrupt sidx"),
            "{error:?}"
        );
    }

    #[test]
    fn static_presentation_plans_everything_and_ends() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT6S">
  <Period>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
      <Representation id="a" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Audio, LiveStartPolicy::LiveEdge, 0);
        let outcome = fixture.plan(mpd);
        assert_eq!(
            media(&outcome)
                .iter()
                .map(|(msn, _, _)| *msn)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(outcome.terminal, Some(TerminalCause::End));
        assert_eq!(
            outcome.prune,
            Some(PruneWindow {
                start: 0,
                len: usize::MAX
            }),
            "static backlog is never dropped"
        );
    }

    #[test]
    fn availability_end_time_ends_the_stream_after_the_last_segments() {
        let mpd = number_mpd(r#"availabilityEndTime="2024-01-01T00:00:12Z""#);
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 9);
        let first = fixture.plan(&mpd);
        assert_eq!(media(&first)[0].1.number, 4);
        assert!(first.terminal.is_none());
        fixture.consume_all();
        fixture.advance(30);
        let last = fixture.plan(&mpd);
        let numbers: Vec<u64> = media(&last)
            .iter()
            .map(|(_, meta, _)| meta.number)
            .collect();
        assert_eq!(numbers, [5, 6]);
        assert_eq!(last.terminal, Some(TerminalCause::End));
    }

    #[test]
    fn failed_snapshots_are_not_planned() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
        fixture.plan(&number_mpd(""));
        let url = Url::parse(MPD_URL).unwrap();
        let snapshot = MpdSnapshot {
            generation: 7,
            manifest_generation: 1,
            manifest: Arc::new(Manifest::parse(number_mpd("").as_bytes(), &url).unwrap()),
            document_url: Arc::new(url),
            parent_params: Vec::new().into(),
            fetched_at: fixture.clock.now(),
            clock_offset: crate::dash::manifest::ClockOffset::ZERO,
            terminal: Some(TerminalCause::Failed(SharedError::new(
                DownloadError::Manifest {
                    reason: "gone".to_owned(),
                },
            ))),
        };
        let planner = fixture.planner.as_mut().unwrap();
        let outcome = planner.plan(&snapshot);
        assert!(outcome.descriptors.is_empty());
        assert_eq!(outcome.terminal, snapshot.terminal);
    }

    fn reissued(outcome: &PlanOutcome) -> Vec<(u64, SegmentKey, String)> {
        outcome
            .descriptors
            .iter()
            .filter(|descriptor| descriptor.key.kind == SegmentKind::Media)
            .map(|descriptor| {
                (
                    descriptor.msn,
                    descriptor.key.clone(),
                    descriptor.parsed_url.to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn an_mpd_update_reissues_unconsumed_work_with_fresh_urls() {
        let with_media = |media: &str| {
            number_mpd("").replace(r#"media="v/$Number$.m4s""#, &format!(r#"media="{media}""#))
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 20);
        let first = reissued(&fixture.plan(&with_media("v/$Number$.m4s")));
        assert_eq!(first.len(), 3);
        // The assembler emitted the first segment; two are still queued.
        fixture.frontier.advance_to(1);

        let update = fixture.plan(&with_media("edge2/v/$Number$.m4s"));
        let again = reissued(&update);
        assert_eq!(
            again.iter().map(|(msn, ..)| *msn).collect::<Vec<_>>(),
            [1, 2],
            "unconsumed work keeps its ordinals; nothing new is available"
        );
        for ((_, key, _), (_, old_key, _)) in again.iter().zip(&first[1..]) {
            assert_eq!(key, old_key, "the store refreshes the record in place");
        }
        assert!(again.iter().all(|(_, _, url)| url.contains("/edge2/v/")));
        let init = init(&update);
        assert!(init.parsed_url.as_str().contains("/v/init.mp4"));

        // Republishing the same MPD issues nothing again.
        assert!(reissued(&fixture.plan(&with_media("edge2/v/$Number$.m4s"))).is_empty());
    }

    #[test]
    fn an_init_change_under_the_same_ids_opens_a_new_epoch() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
        let first = fixture.plan(&number_mpd(""));
        let first_init = init(&first).key.clone();
        fixture.consume_all();
        fixture.advance(2);
        let changed = number_mpd("").replace(
            r#"initialization="v/init.mp4""#,
            r#"initialization="v/init-2.mp4""#,
        );
        let next = fixture.plan(&changed);
        let planned = media(&next);
        assert_eq!(
            planned.iter().map(|(msn, ..)| *msn).collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(planned[0].1.number, 11, "continues after the last segment");
        let media_init = next
            .descriptors
            .iter()
            .find(|descriptor| descriptor.msn == 1 && descriptor.key.kind == SegmentKind::Media)
            .and_then(|descriptor| descriptor.init_key.clone())
            .unwrap();
        assert_ne!(media_init, first_init);
        assert!(media_init.uri.contains("init-2.mp4"));

        // A new presentation time offset is a new epoch too, even with an
        // unchanged init URL: downstream must see a new init to re-time.
        fixture.consume_all();
        fixture.advance(2);
        let shifted = changed.replace(
            r#"duration="2000" timescale="1000" startNumber="1"/>
      <Representation id="v1""#,
            r#"duration="2000" timescale="1000" startNumber="1" presentationTimeOffset="5000"/>
      <Representation id="v1""#,
        );
        assert_ne!(shifted, changed);
        let third = fixture.plan(&shifted);
        let third_init = third
            .descriptors
            .iter()
            .find(|descriptor| descriptor.key.kind == SegmentKind::Init)
            .unwrap();
        assert_eq!(third_init.key.uri, media_init.uri);
        assert_ne!(third_init.key, media_init, "same URL, new epoch scope");
    }

    fn many_periods(count: usize) -> String {
        let periods: String = (0..count)
            .map(|index| {
                format!(
                    r#"<Period id="p{index}" duration="PT2S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p{index}/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>"#
                )
            })
            .collect();
        format!(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">{periods}</MPD>"#)
    }

    #[test]
    fn epochs_are_admitted_one_ahead_of_consumption() {
        let mpd = many_periods(10);
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let mut inits = 0;
        let mut planned = Vec::new();
        for _ in 0..20 {
            let outcome = fixture.plan(&mpd);
            let opened = outcome
                .descriptors
                .iter()
                .filter(|descriptor| descriptor.key.kind == SegmentKind::Init)
                .count();
            assert!(opened <= 2, "at most the current and the next epoch");
            inits += opened;
            planned.extend(media(&outcome).iter().map(|(msn, ..)| *msn));
            if outcome.terminal == Some(TerminalCause::End) {
                break;
            }
            fixture.consume_all();
        }
        assert_eq!(inits, 10);
        assert_eq!(planned, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn huge_windows_are_planned_in_pages() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="P365D">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let first = fixture.plan(mpd);
        assert_eq!(media(&first).len(), PLANNING_LOOKAHEAD as usize);
        assert!(first.terminal.is_none());
        assert!(media(&fixture.plan(mpd)).is_empty(), "the window is full");
        fixture.frontier.advance_to(10);
        let next = media(&fixture.plan(mpd))
            .iter()
            .map(|(msn, ..)| *msn)
            .collect::<Vec<_>>();
        assert_eq!(next, (256..266).collect::<Vec<_>>());
    }

    #[test]
    fn a_later_unusable_period_fails_typed_after_the_earlier_work() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="clear" duration="PT4S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="c/$Number$.m4s" initialization="c/init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="drm" duration="PT4S">
    <AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <SegmentTemplate media="d/$Number$.m4s" initialization="d/init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let first = fixture.plan(mpd);
        assert_eq!(media(&first).len(), 2);
        assert!(
            first.terminal.is_none(),
            "the clear Period is delivered first"
        );
        fixture.consume_all();
        let failed = fixture.plan(mpd);
        let Some(TerminalCause::Failed(error)) = failed.terminal else {
            panic!("expected a failure, got {:?}", failed.terminal);
        };
        let error = error.to_error();
        assert!(
            matches!(
                error,
                DownloadError::Unsupported {
                    protocol: "dash",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn the_start_is_decided_once_for_every_track() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p0/$Number$.m4s" initialization="v.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="p0/a$Number$.m4s" initialization="a.mp4" duration="2" timescale="1"/>
      <Representation id="a" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p1/$Number$.m4s" initialization="v.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let url = Url::parse(MPD_URL).unwrap();
        let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
        // 12 s in, 6 s behind: the start lies in p0, which has audio, even
        // though the newest Period that has begun (p1) has none.
        let start = decide_start(
            &manifest,
            ast() + chrono::TimeDelta::seconds(12),
            LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(6))),
            &DashSelectionConfig::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(start.period.id.as_deref(), Some("p0"));
        assert_eq!(start.after_nanos, Some(6 * NANOS_PER_SEC));
    }

    #[test]
    fn reissue_looks_pending_segments_up_without_scanning_the_gap() {
        let timeline = |entries: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1000001S">
  <Period id="p0">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let first = fixture.plan(&timeline(r#"<S t="0" d="1"/><S t="1000000000" d="1"/>"#));
        assert_eq!(media(&first).len(), 2);
        // The update fills the gap with a billion one-tick segments.
        let started = std::time::Instant::now();
        let update = fixture.plan(&timeline(r#"<S t="0" d="1" r="1000000000"/>"#));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refresh work is proportional to the pending segments"
        );
        let mut reissued: Vec<(u64, u64)> = media(&update)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.time))
            .collect();
        reissued.sort_unstable();
        assert_eq!(reissued, [(0, 0), (1, 1_000_000_000)]);
    }

    const CLEAR_VIDEO: &str = r#"<AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="2" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>"#;
    const CLEAR_AUDIO: &str = r#"<AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
      <Representation id="a" bandwidth="1000"/>
    </AdaptationSet>"#;
    const PROTECTED_AUDIO: &str = r#"<AdaptationSet mimeType="audio/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <SegmentTemplate media="pa/$Number$.m4s" initialization="pa/init.mp4" duration="2" timescale="1"/>
      <Representation id="pa" bandwidth="2000"/>
    </AdaptationSet>"#;
    const PROTECTED_VIDEO: &str = r#"<AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <SegmentTemplate media="pv/$Number$.m4s" initialization="pv/init.mp4" duration="2" timescale="1"/>
      <Representation id="pv" bandwidth="2000"/>
    </AdaptationSet>"#;

    fn static_periods(periods: &[&str]) -> String {
        let periods: String = periods
            .iter()
            .enumerate()
            .map(|(index, sets)| {
                format!(r#"<Period id="p{index}" duration="PT4S">{sets}</Period>"#)
            })
            .collect();
        format!(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">{periods}</MPD>"#)
    }

    #[test]
    fn rejections_count_only_against_their_own_track() {
        let second = format!("{CLEAR_AUDIO}{PROTECTED_AUDIO}");
        let first = format!("{CLEAR_VIDEO}{CLEAR_AUDIO}");
        let mpd = static_periods(&[&first, &second]);
        let url = Url::parse(MPD_URL).unwrap();
        let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
        let later = &manifest.periods[1];
        assert_eq!(rejection_for(later, MediaTrack::Video), None);
        assert!(rejection_for(later, MediaTrack::Audio).is_some());

        // The video track is merely absent from the second Period.
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let outcome = fixture.plan(&mpd);
        assert_eq!(media(&outcome).len(), 2);
        assert_eq!(outcome.terminal, Some(TerminalCause::End));
    }

    #[test]
    fn a_static_presentation_starts_at_its_first_period_even_if_unusable() {
        let first = format!("{PROTECTED_VIDEO}{CLEAR_AUDIO}");
        let second = format!("{CLEAR_VIDEO}{CLEAR_AUDIO}");
        let mpd = static_periods(&[&first, &second]);
        let url = Url::parse(MPD_URL).unwrap();
        let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
        let start = decide_start(
            &manifest,
            ast(),
            LiveStartPolicy::LiveEdge,
            &DashSelectionConfig::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(start.period.id.as_deref(), Some("p0"));
        assert!(matches!(
            rejection_for(&manifest.periods[0], MediaTrack::Video),
            Some(UnsupportedFeature::ContentProtection)
        ));
    }

    #[test]
    fn rejected_text_image_and_metadata_belong_to_no_media_track() {
        let sets = [
            r#"<AdaptationSet contentType="text" mimeType="text/vtt"><Representation id="t1" bandwidth="1"><BaseURL>t.vtt</BaseURL></Representation></AdaptationSet>"#,
            r#"<AdaptationSet mimeType="application/mp4" codecs="stpp"><Representation id="t2" bandwidth="1"><BaseURL>t.mp4</BaseURL></Representation></AdaptationSet>"#,
            r#"<AdaptationSet contentType="image" mimeType="image/jpeg"><Representation id="i" bandwidth="1"><BaseURL>i.jpg</BaseURL></Representation></AdaptationSet>"#,
            r#"<AdaptationSet mimeType="application/x-unknown"><Representation id="u" bandwidth="1"><BaseURL>u</BaseURL></Representation></AdaptationSet>"#,
        ];
        let mpd = static_periods(&[&format!("{CLEAR_VIDEO}{}", sets.concat())]);
        let url = Url::parse(MPD_URL).unwrap();
        let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
        let contents: Vec<RejectedContent> = manifest.periods[0]
            .rejected
            .iter()
            .map(|rejected| rejected.content)
            .collect();
        assert_eq!(
            contents,
            [
                RejectedContent::Other,
                RejectedContent::Other,
                RejectedContent::Other,
                RejectedContent::Unknown
            ]
        );
        let known = static_periods(&[&format!("{CLEAR_VIDEO}{}", sets[..3].concat())]);
        let manifest = Manifest::parse(known.as_bytes(), &url).unwrap();
        assert_eq!(rejection_for(&manifest.periods[0], MediaTrack::Audio), None);
        assert_eq!(rejection_for(&manifest.periods[0], MediaTrack::Video), None);
    }

    #[test]
    fn a_video_gap_is_announced_only_after_overlapping_video_is_planned() {
        // p0 ends at 10 s with 4 s video segments, so its last one, [8, 12),
        // becomes available only at 12 s; p1 (audio only) starts at 10 s.
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT1S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="4" timescale="1"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let absent = |outcome: &PlanOutcome| -> Vec<u64> {
            outcome
                .notices
                .iter()
                .filter_map(|notice| match notice {
                    EngineNotice::TrackAbsent { after_sequence, .. } => Some(*after_sequence),
                    _ => None,
                })
                .collect()
        };
        let mut fixture = Fixture::new(
            MediaTrack::Video,
            LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(11))),
            11,
        );
        let early = fixture.plan(mpd);
        assert_eq!(
            media(&early)
                .iter()
                .map(|(_, meta, _)| meta.number)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(
            absent(&early).is_empty(),
            "segment 3 of p0 is still to come"
        );
        fixture.consume_all();
        fixture.advance(1);
        let complete = fixture.plan(mpd);
        assert_eq!(
            media(&complete)
                .iter()
                .map(|(msn, meta, _)| (*msn, meta.number))
                .collect::<Vec<_>>(),
            [(2, 3)]
        );
        assert_eq!(absent(&complete), [3], "after every p0 video segment");
    }

    #[test]
    fn a_period_covered_exactly_by_fractional_segments_is_complete() {
        // Nine 100100/30000 s segments end exactly at 30.03 s, where summing
        // separately truncated nanoseconds would fall 1 ns short.
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="p0" duration="PT30.03S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="100100" timescale="30000"/>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="p1" duration="PT2S">
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let outcome = fixture.plan(mpd);
        assert_eq!(media(&outcome).len(), 9);
        let absent: Vec<u64> = outcome
            .notices
            .iter()
            .filter_map(|notice| match notice {
                EngineNotice::TrackAbsent { after_sequence, .. } => Some(*after_sequence),
                _ => None,
            })
            .collect();
        assert_eq!(absent, [9], "p1 is announced after p0's nine segments");
    }

    #[test]
    fn segment_bounds_floor_negative_offsets_exactly() {
        // PTO 2 ticks at 3 ticks/s: the first segment starts 2/3 s before
        // the Period (at 10 s) and ends 2/3 s after it.
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT20S">
  <Period id="p0" start="PT10S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="3" presentationTimeOffset="2">
        <SegmentTimeline><S t="0" d="4"/><S d="4"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let url = Url::parse(MPD_URL).unwrap();
        let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
        let period = &manifest.periods[0];
        let representation = &period.adaptation_sets[0].representations[0];
        let window = representation.segments(ast()).unwrap();
        let bounds: Vec<(i128, i128)> = window
            .iter()
            .map(|segment| segment_bounds(&segment.unwrap(), representation, period))
            .collect();
        assert_eq!(
            bounds,
            [
                // 10 s − 666 666 666.67 ns floors to −…667; +666 666 666.67 to …666.
                (9_333_333_333, 10_666_666_666),
                (10_666_666_666, 12_000_000_000),
            ]
        );
    }

    /// Consecutive live MPDs from a TikTok origin (sanitised; see the
    /// fixtures' `served` comments for each response's `Date`). Its edges
    /// alternate between two descriptions of the same stream: one moves
    /// `availabilityStartTime` to the response time and lists only the
    /// newest segment at `t="0"`; the other lists real media times that
    /// lie ~45 minutes past its `availabilityStartTime`. Numbers agree.
    const TIKTOK_LIVE: [&str; 18] = [
        include_str!("../../tests/fixtures/dash/tiktok-live/01.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/02.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/03.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/04.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/05.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/06.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/07.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/08.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/09.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/10.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/11.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/12.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/13.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/14.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/15.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/16.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/17.mpd"),
        include_str!("../../tests/fixtures/dash/tiktok-live/18.mpd"),
    ];

    /// The server time a fixture MPD was served at.
    fn served(mpd: &str) -> DateTime<Utc> {
        let (_, rest) = mpd.split_once("<!-- served: ").expect("served comment");
        let (time, _) = rest.split_once(" -->").expect("served comment end");
        DateTime::parse_from_rfc3339(time)
            .expect("served time")
            .with_timezone(&Utc)
    }

    /// `(ordinal, number)` of every media descriptor, checking that a
    /// reissued ordinal keeps its number.
    fn record_numbers(planned: &mut std::collections::BTreeMap<u64, u64>, outcome: &PlanOutcome) {
        for (msn, meta, _) in media(outcome) {
            if let Some(previous) = planned.insert(msn, meta.number) {
                assert_eq!(previous, meta.number, "ordinal {msn} changed its segment");
            }
        }
    }

    fn init_scopes(outcome: &PlanOutcome) -> Vec<Option<Arc<str>>> {
        outcome
            .descriptors
            .iter()
            .filter(|descriptor| descriptor.key.kind == SegmentKind::Init)
            .map(|descriptor| descriptor.key.scope.clone())
            .collect()
    }

    #[test]
    fn tiktok_live_updates_are_followed_by_segment_number() {
        for track in [MediaTrack::Video, MediaTrack::Audio] {
            let mut fixture = Fixture::new(track, LiveStartPolicy::default(), 0);
            let mut planned = std::collections::BTreeMap::new();
            let mut scopes = std::collections::HashSet::new();
            let mut anchors = Vec::new();
            for mpd in TIKTOK_LIVE {
                fixture
                    .clock
                    .set(served(mpd) + chrono::TimeDelta::milliseconds(500));
                let outcome = fixture.plan(mpd);
                assert!(
                    outcome.terminal.is_none(),
                    "{track}: {:?}",
                    outcome.terminal
                );
                record_numbers(&mut planned, &outcome);
                scopes.extend(init_scopes(&outcome));
                anchors.extend(
                    media(&outcome)
                        .iter()
                        .map(|(msn, meta, _)| (*msn, wall_clock(meta))),
                );
                fixture.consume_all();
            }
            // Every number once, in order, under consecutive ordinals: 1330
            // is only ever listed by the far-ahead description.
            let ordinals: Vec<u64> = planned.keys().copied().collect();
            let numbers: Vec<u64> = planned.values().copied().collect();
            assert_eq!(ordinals, (0..13).collect::<Vec<_>>(), "{track}");
            assert_eq!(numbers, (1319..=1331).collect::<Vec<_>>(), "{track}");
            assert_eq!(scopes.len(), 1, "{track}: one epoch, one init");
            let planner = fixture.planner.as_ref().unwrap();
            assert_eq!(
                planner.reported_conflicts,
                TimingConflict::AnchorMoved as u8
                    | TimingConflict::AheadOfClock as u8
                    | TimingConflict::TimesRewritten as u8
            );
            // Wall-clock anchoring stops with the first contradiction.
            assert!(anchors.first().is_some_and(|(_, anchor)| anchor.is_some()));
            assert!(anchors.last().is_some_and(|(_, anchor)| anchor.is_none()));
        }
    }

    /// A video-only `$Number$` timeline shaped like TikTok's: 2 s segments
    /// (`t` in milliseconds) from `start_number`, published with
    /// `availabilityStartTime` `ast_secs` after [`ast`].
    fn number_timeline(ast_secs: i64, start_number: u64, times: &[u64]) -> String {
        timeline_variant(ast_secs, start_number, times, "v/init.mp4", "v")
    }

    /// [`number_timeline`] with another initialization URL or
    /// Representation id.
    fn timeline_variant(
        ast_secs: i64,
        start_number: u64,
        times: &[u64],
        init: &str,
        representation: &str,
    ) -> String {
        relocated_timeline(ast_secs, start_number, times, init, "v", representation)
    }

    /// [`timeline_variant`] with its media under `media_dir`.
    fn relocated_timeline(
        ast_secs: i64,
        start_number: u64,
        times: &[u64],
        init: &str,
        media_dir: &str,
        representation: &str,
    ) -> String {
        let ast = (ast() + chrono::TimeDelta::seconds(ast_secs)).to_rfc3339();
        let entries: String = times
            .iter()
            .map(|time| format!(r#"<S t="{time}" d="2000"/>"#))
            .collect();
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" minimumUpdatePeriod="PT1S"
     timeShiftBufferDepth="PT12S" availabilityStartTime="{ast}" publishTime="{ast}">
  <Period id="dash" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="{media_dir}/$Number$.m4s" initialization="{init}" timescale="1000"
          availabilityTimeOffset="1.95" startNumber="{start_number}">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="{representation}" bandwidth="1000000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
        )
    }

    fn planned_numbers(outcome: &PlanOutcome) -> Vec<(u64, u64)> {
        media(outcome)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.number))
            .collect()
    }

    #[test]
    fn contradicting_updates_never_regress_or_replan() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        let at = |fixture: &Fixture, secs: i64| {
            fixture
                .clock
                .set(ast() + chrono::TimeDelta::milliseconds(secs * 1000 + 500));
        };
        // The newest segment starts at the anchor, as the response time.
        at(&fixture, 100);
        let first = fixture.plan(&number_timeline(100, 50, &[0]));
        assert_eq!(planned_numbers(&first), [(0, 50)]);
        assert_eq!(
            wall_clock(media(&first)[0].1),
            Some(ast() + chrono::TimeDelta::seconds(100))
        );

        // Real media times, ~16 minutes past the anchor: followed anyway.
        at(&fixture, 101);
        let ahead = fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
        assert_eq!(
            planned_numbers(&ahead),
            [(0, 50), (1, 51)],
            "50 is reissued"
        );
        assert!(
            media(&ahead)
                .iter()
                .all(|(_, meta, _)| wall_clock(meta).is_none()),
            "neither new nor reissued work keeps the contradicted anchor"
        );
        assert_eq!(
            wall_clock(media(&first)[0].1),
            None,
            "work planned before the contradiction loses it too"
        );

        // `t` back at 0 for an already planned number: nothing new.
        at(&fixture, 102);
        let behind = fixture.plan(&number_timeline(102, 51, &[0]));
        assert!(planned_numbers(&behind).iter().all(|(msn, _)| *msn < 2));
        assert!(behind.terminal.is_none());

        // The next number, again at `t="0"`.
        at(&fixture, 103);
        fixture.consume_all();
        let next = fixture.plan(&number_timeline(103, 52, &[0]));
        assert_eq!(planned_numbers(&next), [(2, 52)]);

        // The anchor moves backwards and the times forwards again.
        at(&fixture, 104);
        fixture.consume_all();
        let later = fixture.plan(&number_timeline(
            100,
            51,
            &[1_002_000, 1_004_000, 1_006_000],
        ));
        assert_eq!(planned_numbers(&later), [(3, 53)]);

        let scopes: std::collections::HashSet<_> = [first, ahead, behind, next, later]
            .iter()
            .flat_map(init_scopes)
            .collect();
        assert_eq!(scopes.len(), 1, "one epoch throughout");
        assert_eq!(
            fixture.planner.as_ref().unwrap().reported_conflicts,
            TimingConflict::AnchorMoved as u8
                | TimingConflict::AheadOfClock as u8
                | TimingConflict::TimesRewritten as u8
        );
    }

    #[test]
    fn a_number_timeline_whose_entries_expire_is_followed_by_number() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // Consistent with the clock: segment n (from 1) ends at 2n s.
        fixture
            .clock
            .set(ast() + chrono::TimeDelta::milliseconds(8_100));
        let first = fixture.plan(&number_timeline(0, 1, &[0, 2000, 4000, 6000]));
        assert_eq!(planned_numbers(&first), [(0, 4)]);
        fixture.consume_all();
        fixture.advance(4);
        // Two entries expired; `startNumber` follows.
        let next = fixture.plan(&number_timeline(0, 3, &[4000, 6000, 8000, 10000]));
        assert_eq!(planned_numbers(&next), [(1, 5), (2, 6)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
        assert!(
            media(&next)
                .iter()
                .all(|(_, meta, _)| wall_clock(meta) == Some(ast()))
        );
    }

    #[test]
    fn a_time_template_timeline_stays_keyed_by_time() {
        let timeline = |start_number: u64, entries: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000" startNumber="{start_number}">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v1" bandwidth="1000000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        let first = fixture.plan(&timeline(1, r#"<S t="0" d="2000" r="4"/>"#));
        let times: Vec<(u64, u64)> = media(&first)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.time))
            .collect();
        assert_eq!(times, [(0, 8000)]);
        fixture.consume_all();
        fixture.advance(4);
        // The numbers jump; the media times carry on and decide.
        let next = fixture.plan(&timeline(100, r#"<S t="6000" d="2000" r="3"/>"#));
        let times: Vec<(u64, u64)> = media(&next)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.time))
            .collect();
        assert_eq!(times, [(1, 10000), (2, 12000)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    #[test]
    fn a_timeline_listing_the_segment_in_production_waits_for_the_clock() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // Available segments end by 9 + 1.95 s: 1–5 end at 2–10 s.
        fixture.clock.set(ast() + chrono::TimeDelta::seconds(9));
        let mpd = number_timeline(0, 1, &[0, 2000, 4000, 6000, 8000]);
        let first = fixture.plan(&mpd);
        assert_eq!(planned_numbers(&first), [(0, 5)]);
        fixture.consume_all();
        let mpd = number_timeline(0, 1, &[0, 2000, 4000, 6000, 8000, 10000]);
        let early = fixture.plan(&mpd);
        // Segment 6 ends 1.05 s past the window: a segment in production,
        // not a contradiction.
        assert!(media(&early).is_empty());
        fixture.advance(2);
        let next = fixture.plan(&mpd);
        assert_eq!(planned_numbers(&next), [(1, 6)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    /// Sets the clock `millis` after [`ast`].
    fn set_clock(fixture: &Fixture, millis: i64) {
        fixture
            .clock
            .set(ast() + chrono::TimeDelta::milliseconds(millis));
    }

    #[test]
    fn a_first_mpd_far_ahead_of_the_clock_waits_for_a_consistent_one() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // One MPD cannot show that its timing is contradicted: the clock
        // decides, and nothing listed is available by it.
        set_clock(&fixture, 500);
        let ahead = fixture.plan(&number_timeline(
            0,
            1321,
            &[2_669_356, 2_671_356, 2_673_356],
        ));
        assert!(media(&ahead).is_empty());
        // The next refresh describes the newest segment as starting now.
        set_clock(&fixture, 1_500);
        let consistent = fixture.plan(&number_timeline(1, 1324, &[0]));
        assert_eq!(planned_numbers(&consistent), [(0, 1324)]);
        fixture.consume_all();
        // Once the anchor moved, the far-ahead description is followed.
        set_clock(&fixture, 2_500);
        let followed = fixture.plan(&number_timeline(2, 1324, &[2_675_356, 2_677_356]));
        assert_eq!(planned_numbers(&followed), [(1, 1325)]);
        assert_eq!(wall_clock(media(&followed)[0].1), None);
    }

    #[test]
    fn an_anchor_moving_between_far_ahead_descriptions_lets_them_through() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // An origin that only ever describes media far ahead of an anchor
        // it moves: the second description shows the contradiction.
        set_clock(&fixture, 500);
        let first = fixture.plan(&number_timeline(0, 1321, &[2_669_356, 2_671_356]));
        assert!(media(&first).is_empty());
        set_clock(&fixture, 1_500);
        let second = fixture.plan(&number_timeline(1, 1322, &[2_671_356, 2_673_356]));
        // Nothing was available at the start, so whatever is listed now.
        assert_eq!(planned_numbers(&second), [(0, 1322), (1, 1323)]);
        assert_eq!(wall_clock(media(&second)[0].1), None);
    }

    #[test]
    fn times_rewritten_under_a_fixed_anchor_let_far_ahead_descriptions_through() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // #50 starts at the anchor, then the same anchor describes #50 and
        // #51 ~16 minutes later: nothing the clock calls available shows
        // the contradiction, the listing does.
        set_clock(&fixture, 500);
        let first = fixture.plan(&number_timeline(0, 50, &[0]));
        assert_eq!(planned_numbers(&first), [(0, 50)]);
        fixture.consume_all();
        set_clock(&fixture, 1_500);
        let rewritten = fixture.plan(&number_timeline(0, 50, &[1_000_000, 1_002_000]));
        assert_eq!(planned_numbers(&rewritten), [(1, 51)]);
        assert_eq!(
            fixture.planner.as_ref().unwrap().reported_conflicts,
            TimingConflict::TimesRewritten as u8 | TimingConflict::AheadOfClock as u8
        );
    }

    #[test]
    fn a_running_period_listing_only_future_segments_waits_for_the_clock() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        set_clock(&fixture, 4_500);
        let first = fixture.plan(&number_timeline(0, 1, &[0, 2000]));
        assert_eq!(planned_numbers(&first), [(0, 2)]);
        fixture.consume_all();
        // After a gap, only a segment from 100 s on is listed. Nothing
        // contradicts the timing, so it is not requested before then.
        set_clock(&fixture, 20_000);
        let mpd = number_timeline(0, 3, &[100_000]);
        assert!(media(&fixture.plan(&mpd)).is_empty());
        set_clock(&fixture, 98_000);
        assert!(media(&fixture.plan(&mpd)).is_empty());
        set_clock(&fixture, 100_100);
        assert_eq!(planned_numbers(&fixture.plan(&mpd)), [(1, 3)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    #[test]
    fn a_switch_to_another_representation_never_compares_numbers() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // `v` #51 covers 8–10 s.
        set_clock(&fixture, 10_500);
        let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
        assert_eq!(planned_numbers(&first), [(0, 51)]);
        fixture.consume_all();
        // `w` numbers the same interval differently: #50 is 8–10 s, #51 is
        // 10–12 s. That is no rewritten time, and `w` #51 is next.
        set_clock(&fixture, 12_500);
        let switched = fixture.plan(&steady_timeline(50, &[8000, 10000], "w/init.mp4", "w"));
        let planned: Vec<(u64, u64, u64)> = media(&switched)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.number, meta.time))
            .collect();
        assert_eq!(planned, [(1, 51, 10000)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    #[test]
    fn a_moved_anchor_reaches_superseded_epochs() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        set_clock(&fixture, 10_500);
        let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
        assert_eq!(planned_numbers(&first), [(0, 51)]);
        assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
        // #51 is still buffered when an update changes both the
        // initialization and the anchor, with media times moving on.
        set_clock(&fixture, 12_500);
        let next = fixture.plan(&timeline_variant(1, 51, &[8000, 10000], "v/init2.mp4", "v"));
        assert_eq!(planned_numbers(&next), [(1, 52)]);
        assert_eq!(wall_clock(media(&first)[0].1), None);
        assert_eq!(wall_clock(media(&next)[0].1), None);
    }

    /// A live MPD at 20 s whose current Period `p0` runs to 100 s, with
    /// `p1` preannounced from 100 s.
    fn preannounced_period(media: &str) -> String {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p0/{media}" initialization="p0/init.mp4" timescale="1">
        <SegmentTimeline><S t="0" d="2" r="49"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
  <Period id="p1" start="PT100S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p1/{media}" initialization="p1/init.mp4" timescale="1">
        <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
        )
    }

    #[test]
    fn a_preannounced_period_never_becomes_the_live_edge() {
        for media in ["$Number$.m4s", "$Time$.m4s"] {
            let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
            let mpd = preannounced_period(media);
            let first = fixture.plan(&mpd);
            let planned: Vec<(Option<&str>, u64, u64)> = media_of(&first);
            // The segment ending at 20 s: number 10, time 18.
            assert_eq!(planned, [(Some("p0"), 10, 18)], "{media}");
            fixture.consume_all();
            fixture.advance(2);
            let next = fixture.plan(&mpd);
            assert_eq!(media_of(&next), [(Some("p0"), 11, 20)], "{media}");
            assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
        }
    }

    fn media_of(outcome: &PlanOutcome) -> Vec<(Option<&str>, u64, u64)> {
        media(outcome)
            .iter()
            .map(|(_, meta, _)| (meta.period_id.as_deref(), meta.number, meta.time))
            .collect()
    }

    #[test]
    fn a_timeline_scheduling_future_segments_is_not_trusted_over_the_clock() {
        let timeline = |media: &str, entries: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/{media}" initialization="v/init.mp4" timescale="1">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        // Listed up to 200 s at 20 s: the clock decides, whatever the key.
        for media in ["$Number$.m4s", "$Time$.m4s"] {
            let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
            let outcome = fixture.plan(&timeline(media, r#"<S t="0" d="2" r="99"/>"#));
            assert_eq!(media_of(&outcome), [(Some("p0"), 10, 18)], "{media}");
            assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
        }
        // Everything listed lies ahead, but `$Time$` URLs are built from
        // those very times: wait for the clock.
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
        let outcome = fixture.plan(&timeline("$Time$.m4s", r#"<S t="1000" d="2" r="4"/>"#));
        assert!(media(&outcome).is_empty());
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    #[test]
    fn epoch_changes_during_contradicting_updates_continue_by_number() {
        let variants = [("v/init2.mp4", "v"), ("v/init.mp4", "v2")];
        for (init, representation) in variants {
            let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
            let at = |fixture: &Fixture, secs: i64| {
                fixture
                    .clock
                    .set(ast() + chrono::TimeDelta::milliseconds(secs * 1000 + 500));
            };
            at(&fixture, 100);
            let first = fixture.plan(&number_timeline(100, 50, &[0]));
            fixture.consume_all();
            at(&fixture, 101);
            let ahead = fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
            assert_eq!(planned_numbers(&ahead), [(1, 51)]);
            fixture.consume_all();
            // A new init or Representation, described at `t="0"` again.
            at(&fixture, 102);
            let switched = fixture.plan(&timeline_variant(102, 52, &[0], init, representation));
            assert_eq!(
                planned_numbers(&switched),
                [(2, 52)],
                "{init} {representation}"
            );
            let scopes: std::collections::HashSet<_> = [first, ahead, switched]
                .iter()
                .flat_map(init_scopes)
                .collect();
            assert_eq!(scopes.len(), 2, "a new epoch");
            fixture.consume_all();
            // The new epoch keeps following numbers across descriptions.
            at(&fixture, 103);
            let later = fixture.plan(&timeline_variant(
                103,
                52,
                &[1_004_000, 1_006_000],
                init,
                representation,
            ));
            assert_eq!(planned_numbers(&later), [(3, 53)]);
            assert!(
                media(&later)
                    .iter()
                    .all(|(_, meta, _)| wall_clock(meta).is_none())
            );
        }
    }

    /// A video `$Number$` timeline consistent with the clock: 2 s segments
    /// from `start_number` at the given media times (ms), anchored at
    /// [`ast`].
    fn steady_timeline(
        start_number: u64,
        times: &[u64],
        init: &str,
        representation: &str,
    ) -> String {
        timeline_variant(0, start_number, times, init, representation)
    }

    #[test]
    fn a_switch_that_first_contradicts_the_media_times_continues_by_number() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
        // #50 and #51 end at 8 and 10 s.
        let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
        assert_eq!(planned_numbers(&first), [(0, 51)]);
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
        fixture.consume_all();
        // The initialization changes and #52 is described at `t="0"`, with
        // the anchor unchanged: the first contradiction is at the switch.
        let switched = steady_timeline(52, &[0], "v/init2.mp4", "v");
        let outcome = fixture.plan(&switched);
        assert_eq!(planned_numbers(&outcome), [(1, 52)]);
        assert_ne!(init_scopes(&first), init_scopes(&outcome), "a new epoch");
        assert_eq!(wall_clock(media(&first)[0].1), None);
        assert_eq!(wall_clock(media(&outcome)[0].1), None);
        assert_eq!(
            fixture.planner.as_ref().unwrap().reported_conflicts,
            TimingConflict::TimesRewritten as u8
        );
        fixture.consume_all();
        // Repeated, it offers nothing new; the next number follows.
        assert!(media(&fixture.plan(&switched)).is_empty());
        let next = fixture.plan(&steady_timeline(52, &[0, 2000], "v/init2.mp4", "v"));
        assert_eq!(planned_numbers(&next), [(2, 53)]);
    }

    #[test]
    fn a_switch_to_independent_numbers_starts_like_a_recording() {
        // Another Representation's own numbering: far below the previous
        // newest (#51), or overlapping it for a different interval.
        for (start_number, expected) in [(1, [4, 5, 6]), (48, [51, 52, 53])] {
            let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
            // Contradicting descriptions establish distrust.
            set_clock(&fixture, 100_500);
            fixture.plan(&number_timeline(100, 50, &[0]));
            fixture.consume_all();
            set_clock(&fixture, 101_500);
            fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
            fixture.consume_all();
            // Six segments ending at 2–12 s after an anchor 12.5 s ago.
            set_clock(&fixture, 112_500);
            let switched = fixture.plan(&timeline_variant(
                100,
                start_number,
                &[0, 2000, 4000, 6000, 8000, 10000],
                "w/init.mp4",
                "w",
            ));
            // Three segments behind the edge (the default delay): numbers
            // of another sequence say nothing about where #51 left off.
            let numbers: Vec<u64> = planned_numbers(&switched)
                .iter()
                .map(|(_, number)| *number)
                .collect();
            assert_eq!(numbers, expected, "from {start_number}");
        }
    }

    #[test]
    fn media_times_are_compared_across_timescales() {
        let timeline = |init: &str, timescale: u64, start_number: u64, entries: &str| {
            format!(
                r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT1S">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="{init}" timescale="{timescale}" startNumber="{start_number}">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="1000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
            )
        };
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // #51 is 8–10 s at 90 kHz.
        set_clock(&fixture, 10_500);
        let first = fixture.plan(&timeline(
            "v/init.mp4",
            90_000,
            50,
            r#"<S t="540000" d="180000" r="1"/>"#,
        ));
        assert_eq!(planned_numbers(&first), [(0, 51)]);
        fixture.consume_all();
        // A new init at 1 kHz announces #52 at 100 s: later, not rewritten,
        // so nothing is requested before the clock reaches it.
        set_clock(&fixture, 11_000);
        let next = fixture.plan(&timeline(
            "v/init2.mp4",
            1000,
            52,
            r#"<S t="100000" d="2000"/>"#,
        ));
        assert!(media(&next).is_empty());
        assert_eq!(fixture.planner.as_ref().unwrap().reported_conflicts, 0);
    }

    /// A live MPD of `$Number$` timelines in seconds, anchored at [`ast`].
    fn periods_mpd(periods: &[(&str, u64, &str, u64, &str)]) -> String {
        let periods: String = periods
            .iter()
            .map(|(id, start, representation, start_number, entries)| {
                format!(
                    r#"<Period id="{id}" start="PT{start}S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="{id}/{representation}/$Number$.m4s" initialization="{id}/{representation}/init.mp4" timescale="1" startNumber="{start_number}">
        <SegmentTimeline>{entries}</SegmentTimeline>
      </SegmentTemplate>
      <Representation id="{representation}" bandwidth="1000"/>
    </AdaptationSet>
  </Period>"#
                )
            })
            .collect();
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S" timeShiftBufferDepth="PT30S">
  {periods}
</MPD>"#
        )
    }

    #[test]
    fn a_contradiction_in_one_period_leaves_the_next_trusted() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        let p1 = ("p1", 10, "v", 1, r#"<S t="0" d="2" r="4"/>"#);
        set_clock(&fixture, 6_000);
        let first = fixture.plan(&periods_mpd(&[
            ("p0", 0, "v", 1, r#"<S t="0" d="2" r="2"/>"#),
            p1,
        ]));
        assert_eq!(planned_numbers(&first), [(0, 3)]);
        fixture.consume_all();
        // p0 re-describes #3 at another time.
        set_clock(&fixture, 7_000);
        fixture.plan(&periods_mpd(&[
            ("p0", 0, "v", 3, r#"<S t="100" d="2"/>"#),
            p1,
        ]));
        fixture.consume_all();
        // p1 is recorded to 20 s.
        set_clock(&fixture, 20_000);
        let p1_first = fixture.plan(&periods_mpd(&[p1]));
        assert_eq!(
            planned_numbers(&p1_first).last().map(|(_, number)| *number),
            Some(5)
        );
        assert_eq!(
            wall_clock(media(&p1_first)[0].1),
            Some(ast()),
            "p1 stays anchored"
        );
        fixture.consume_all();
        // A Representation switch in p1, whose timing nothing contradicted,
        // continues where p1's coverage ended.
        set_clock(&fixture, 24_000);
        let switched = fixture.plan(&periods_mpd(&[(
            "p1",
            10,
            "w",
            1,
            r#"<S t="0" d="2" r="6"/>"#,
        )]));
        let numbers: Vec<u64> = planned_numbers(&switched)
            .iter()
            .map(|(_, number)| *number)
            .collect();
        assert_eq!(numbers, [6, 7]);
    }

    #[test]
    fn a_contradiction_reaches_retired_epochs_of_its_period() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        set_clock(&fixture, 10_500);
        let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
        assert_eq!(planned_numbers(&first), [(0, 51)]);
        fixture.consume_all();
        // A new initialization: the first epoch is retired once consumed,
        // while #51 may still be buffered downstream.
        set_clock(&fixture, 12_500);
        let second = fixture.plan(&steady_timeline(51, &[8000, 10000], "v/init2.mp4", "v"));
        assert_eq!(planned_numbers(&second), [(1, 52)]);
        fixture.consume_all();
        assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
        // #52 re-described at another time contradicts the Period.
        set_clock(&fixture, 14_500);
        fixture.plan(&steady_timeline(52, &[0, 2000], "v/init2.mp4", "v"));
        assert_eq!(fixture.planner.as_ref().unwrap().epochs.len(), 1);
        assert_eq!(wall_clock(media(&first)[0].1), None);
        assert_eq!(wall_clock(media(&second)[0].1), None);
    }

    #[test]
    fn period_trust_is_forgotten_with_its_periods() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        for index in 0..50_u64 {
            // Each MPD carries the next Period and moves the anchor.
            let ast_secs = index % 2;
            let start = 10 * index;
            let mpd = periods_mpd(&[("p", start, "v", 1, r#"<S t="0" d="2" r="4"/>"#)])
                .replace(r#"id="p""#, &format!(r#"id="p{index}""#))
                .replace(
                    "2024-01-01T00:00:00Z",
                    &(ast() + chrono::TimeDelta::seconds(ast_secs as i64)).to_rfc3339(),
                );
            set_clock(&fixture, (start as i64 + 11) * 1000);
            let outcome = fixture.plan(&mpd);
            assert!(!media(&outcome).is_empty(), "Period {index}");
            fixture.consume_all();
            let periods = fixture.planner.as_ref().unwrap().periods.len();
            assert!(periods <= 3, "{periods} Periods remembered at {index}");
        }
    }

    #[test]
    fn a_cdn_switch_in_a_contradicted_period_continues_by_number() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        set_clock(&fixture, 100_500);
        fixture.plan(&number_timeline(100, 50, &[0]));
        fixture.consume_all();
        set_clock(&fixture, 101_500);
        let ahead = fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
        assert_eq!(planned_numbers(&ahead), [(1, 51)]);
        fixture.consume_all();
        // Another CDN: initialization and media elsewhere, #50–#52 listed.
        set_clock(&fixture, 102_500);
        let moved = fixture.plan(&relocated_timeline(
            102,
            50,
            &[1_000_000, 1_002_000, 1_004_000],
            "edge2/init.mp4",
            "edge2/v",
            "v",
        ));
        // Neither replayed nor skipped.
        assert_eq!(planned_numbers(&moved), [(2, 52)]);
    }

    #[test]
    fn a_stale_mpd_a_few_numbers_behind_is_not_a_restart() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        let mut scopes = std::collections::HashSet::new();
        let steps: [(i64, i64, u64, &[u64]); 4] = [
            (100, 100, 50, &[0]),
            (101, 101, 50, &[1_000_000, 1_002_000]),
            (102, 102, 52, &[0]),
            (103, 100, 51, &[1_002_000, 1_004_000, 1_006_000]),
        ];
        for (clock, ast_secs, start_number, times) in steps {
            set_clock(&fixture, clock * 1000 + 500);
            scopes.extend(init_scopes(&fixture.plan(&number_timeline(
                ast_secs,
                start_number,
                times,
            ))));
            fixture.consume_all();
        }
        assert_eq!(
            fixture.planner.as_ref().unwrap().next_ordinal,
            4,
            "up to #53"
        );
        // Cached copies, their newest number 5 and 10 behind: nothing is
        // planned again and no epoch opens.
        for (start_number, times) in [
            (46, [992_000, 994_000, 996_000]),
            (41, [982_000, 984_000, 986_000]),
        ] {
            set_clock(&fixture, 104_500);
            let stale = fixture.plan(&number_timeline(101, start_number, &times));
            assert!(media(&stale).is_empty(), "from {start_number}");
            scopes.extend(init_scopes(&stale));
        }
        assert_eq!(scopes.len(), 1, "no new epoch");
    }

    /// [`number_timeline`] as one open `S@r=-1` run from `t="0"` with a
    /// 30 s time-shift buffer.
    fn open_timeline(ast_secs: i64, start_number: u64, time: u64) -> String {
        number_timeline(ast_secs, start_number, &[time])
            .replace(r#"d="2000"/>"#, r#"d="2000" r="-1"/>"#)
            .replace(
                r#"timeShiftBufferDepth="PT12S""#,
                r#"timeShiftBufferDepth="PT30S""#,
            )
    }

    #[test]
    fn an_open_timeline_refreshed_for_minutes_plans_each_segment_once() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 100);
        let mpd = open_timeline(0, 1, 0);
        let mut numbers = Vec::new();
        for _ in 0..90 {
            let outcome = fixture.plan(&mpd);
            assert_eq!(outcome.terminal, None);
            numbers.extend(planned_numbers(&outcome).iter().map(|(_, number)| *number));
            fixture.consume_all();
            fixture.advance(2);
        }
        // One new segment per 2 s, each once.
        assert_eq!(numbers, (50..=139).collect::<Vec<_>>());
    }

    #[test]
    fn open_timelines_in_contradicting_descriptions_are_followed_by_number() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        let mut numbers = Vec::new();
        for step in 0..20_u64 {
            let secs = 100 + 2 * step as i64;
            set_clock(&fixture, secs * 1000 + 500);
            // The response-time description of the newest segment, then an
            // open run far ahead of its anchor from a few numbers back.
            let current = 50 + step;
            for mpd in [
                open_timeline(secs, current, 0),
                open_timeline(secs + 1, current.saturating_sub(5), 2_000_000 + 2000 * step),
            ] {
                let outcome = fixture.plan(&mpd);
                assert_eq!(outcome.terminal, None, "step {step}");
                numbers.extend(planned_numbers(&outcome).iter().map(|(_, number)| *number));
                fixture.consume_all();
            }
        }
        numbers.dedup();
        assert_eq!(numbers, (50..=69).collect::<Vec<_>>());
    }

    #[test]
    fn an_epoch_queued_from_before_an_anchor_move_leaves_later_periods_anchored() {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
        // p0's #3 is planned and still queued.
        set_clock(&fixture, 6_000);
        let first = fixture.plan(&periods_mpd(&[(
            "p0",
            0,
            "v",
            1,
            r#"<S t="0" d="2" r="2"/>"#,
        )]));
        assert_eq!(planned_numbers(&first), [(0, 3)]);
        assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
        // The anchor moves by a second, and p1 begins under it.
        set_clock(&fixture, 21_000);
        let moved = periods_mpd(&[("p1", 10, "v", 1, r#"<S t="0" d="2" r="4"/>"#)])
            .replace("2024-01-01T00:00:00Z", "2024-01-01T00:00:01Z");
        let p1 = fixture.plan(&moved);
        let anchor = Some(ast() + chrono::TimeDelta::seconds(1));
        assert!(!media(&p1).is_empty());
        assert_eq!(wall_clock(media(&first)[0].1), None, "p0's anchor moved");
        assert_eq!(wall_clock(media(&p1)[0].1), anchor);
        // Refreshed unchanged while p0's epoch is still queued: p1 keeps its
        // anchor.
        set_clock(&fixture, 21_500);
        fixture.plan(&moved);
        assert_eq!(fixture.planner.as_ref().unwrap().epochs.len(), 2);
        assert_eq!(wall_clock(media(&p1)[0].1), anchor);
    }
}
