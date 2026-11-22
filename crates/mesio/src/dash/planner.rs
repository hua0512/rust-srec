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
//! is not recorded, even once a Period that has begun carries it (a later
//! one, or the starting one after an MPD update); its first appearance is
//! reported with a warning.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use tracing::{debug, info, warn};
use url::Url;

use crate::DownloadError;
use crate::dash::config::{DashSelectionConfig, LiveStartPolicy};
use crate::dash::manifest::{
    AddressingKind, ByteRange, Clock, Manifest, ManifestError, NANOS_PER_SEC, Period,
    PresentationType, RejectedContent, Representation, Segment, SegmentWindow, UnsupportedFeature,
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
    pub(crate) fn at_or_after(&self, other: &Self) -> bool {
        self.same(other) || self.is_after(other)
    }

    /// Whether `self` is another Period, starting before `other`.
    pub(crate) fn is_before(&self, other: &Self) -> bool {
        !self.same(other) && self.start < other.start
    }

    /// Whether `self` is another Period, starting after `other`.
    pub(crate) fn is_after(&self, other: &Self) -> bool {
        !self.same(other) && self.start > other.start
    }

    pub(crate) fn label(&self) -> String {
        match &self.id {
            Some(id) => format!("id:{id}"),
            None => format!("start:{}", self.start.as_nanos()),
        }
    }
}

/// The Period as output descriptions name it.
impl std::fmt::Display for PeriodKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.id {
            Some(id) => write!(f, "period {id}"),
            None => write!(f, "period at {:?}", self.start),
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
    /// Whether segments are addressed by `$Number$` on a `SegmentTimeline`.
    fn is_number_timeline(&self) -> bool {
        self.timeline && self.addressing == AddressingKind::Number
    }

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
    /// `MPD@availabilityStartTime` when the epoch opened, as the wall-clock
    /// anchor every descriptor of the epoch shares, while its timing was not
    /// contradicted.
    anchor: Option<WallClockAnchor>,
}

impl Epoch {
    /// The epoch's Period and Representation as `manifest` describes them,
    /// if it still does.
    fn find_in<'a>(
        &self,
        manifest: &'a Manifest,
        track: MediaTrack,
    ) -> Option<(&'a Period, &'a Representation)> {
        let period = manifest
            .periods
            .iter()
            .find(|period| PeriodKey::of(period).same(&self.period))?;
        let representation = selection::find(period, track, &self.representation_id)?;
        Some((period, representation))
    }
}

/// Whether `representation` addresses its segments by `$Number$` on a
/// `SegmentTimeline`.
fn is_number_timeline(representation: &Representation) -> bool {
    representation.uses_segment_timeline()
        && representation.addressing_kind() == AddressingKind::Number
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
    at: Timestamp,
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
    now: Timestamp,
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
    now: Timestamp,
    overrule: bool,
) -> Result<Availability<'_>, ManifestError> {
    let lead = match overrule {
        true => timeline_contradicts_clock(representation, now)?,
        false => None,
    };
    let at = lead
        .and_then(|lead| SignedDuration::try_from(lead).ok())
        .and_then(|lead| now.checked_add(lead).ok());
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

/// The Period and Representation being planned, with the snapshot they
/// come from.
#[derive(Clone, Copy)]
struct PeriodScope<'a> {
    snapshot: &'a MpdSnapshot,
    period: &'a Period,
    key: &'a PeriodKey,
    representation: &'a Representation,
    /// The instant (server time) the window was evaluated at.
    at: Timestamp,
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
    /// Configured tracks this session does not record, reported (and
    /// removed) once a Period that has begun offers them.
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
    last_anchor: Option<(Option<Timestamp>, Vec<PeriodKey>)>,
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

    /// Notes a dynamic MPD's `availabilityStartTime` moving since the
    /// previous MPD planned. Runs before anything is reissued or planned.
    /// The move withdraws every anchor issued under the old value and
    /// contradicts the Periods both MPDs describe and those of every
    /// retained epoch. Every epoch opens under the value of the MPD that
    /// last passed here, so one opened under an older value has seen such a
    /// move, and its Period stays contradicted while the epoch is retained.
    /// Later MPDs under the new value leave the anchors issued under it in
    /// place, even while an epoch from before the move is still queued.
    fn note_moved_anchor(&mut self, manifest: &Manifest) {
        if manifest.presentation_type != PresentationType::Dynamic {
            return;
        }
        let ast = manifest.availability_start_time;
        let periods: Vec<PeriodKey> = manifest.periods.iter().map(PeriodKey::of).collect();
        let previous = self.last_anchor.replace((ast, periods));
        let Some((from, previous_periods)) = previous.filter(|(from, _)| *from != ast) else {
            return;
        };
        let moved: Vec<PeriodKey> = self
            .last_anchor
            .iter()
            .flat_map(|(_, periods)| periods)
            .filter(|period| previous_periods.iter().any(|key| key.same(period)))
            .chain(self.epochs.iter().map(|epoch| &epoch.period))
            .cloned()
            .collect();
        if moved.is_empty() {
            return;
        }
        self.presentation_withdrawal.withdraw();
        self.presentation_withdrawal = AnchorWithdrawal::default();
        for period in &moved {
            self.record_contradiction(period);
        }
        let from = from.map(|time| time.to_string());
        let to = manifest
            .availability_start_time
            .map(|time| time.to_string());
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
            if !epoch.fingerprint.is_number_timeline() {
                continue;
            }
            let Some((_, representation)) = epoch
                .find_in(manifest, self.track)
                .filter(|(_, representation)| is_number_timeline(representation))
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
        self.note_late_tracks(manifest, &lower, now);
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
        now: Timestamp,
        outcome: &mut PlanOutcome,
    ) -> Result<(), ManifestError> {
        for index in 0..self.epochs.len() {
            let epoch = &self.epochs[index];
            // Without its index (only the selected Representation's is
            // read) the epoch's segments cannot be found again; like a
            // vanished Representation, its work keeps its URLs.
            let Some((period, representation)) = epoch
                .find_in(&snapshot.manifest, self.track)
                .filter(|(_, representation)| representation.pending_segment_index().is_none())
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

    /// Reports a timeline followed ahead of the clock (`scope.lead`). Only a
    /// contradicted Period's timeline is followed so, whose epochs carry no
    /// wall-clock anchor already. Planning is unaffected.
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

    /// Reports each configured track this session does not record once a
    /// Period from `lower` on that has begun offers it, whether a later
    /// Period or an update of one already planned. Runs on every snapshot,
    /// whatever gets planned; each track is reported once.
    fn note_late_tracks(&mut self, manifest: &Manifest, lower: &PeriodKey, now: Timestamp) {
        if self.absent_tracks.is_empty() {
            return;
        }
        for period in &manifest.periods {
            let key = PeriodKey::of(period);
            if !key.at_or_after(lower) || !has_started(manifest, period, now) {
                continue;
            }
            let selection = &self.selection;
            self.absent_tracks.retain(|&absent| {
                if selection::select(period, absent, selection).is_none() {
                    return true;
                }
                warn!(
                    track = %absent,
                    period = %key.label(),
                    "a track appeared after recording started; it is not recorded in this session"
                );
                false
            });
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
        let last = previous.last_locator;
        let same_sequence = fingerprint.is_number_timeline()
            && previous.fingerprint.is_number_timeline()
            && *previous.representation_id == *scope.representation.id;
        let last_end = previous.last_end;
        // A timeline followed ahead of the clock (`scope.lead`) and a moved
        // anchor both contradicted the Period before planning started.
        let trusted = !self.contradicted(scope.key);
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
            .map(|ast| scope.at.duration_since(ast).as_nanos());
        let target = live_start_target(
            self.live_start,
            scope.manifest(),
            (last_start, last_end),
            elapsed,
        );
        first_index(window, |segment| scope.bounds(segment).1 > target)
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
        // Every epoch of a Period shares its withdrawal, so a contradiction
        // seen later also reaches the output of epochs retired by then.
        let trusted = !self.contradicted(key);
        let period_withdrawal = self.trust_mut(key).withdrawal.clone();
        let anchor = scope
            .manifest()
            .availability_start_time
            .filter(|_| trusted)
            .map(|time| {
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
        self.epochs.push_back(Epoch {
            period: key.clone(),
            representation_id: Arc::from(representation.id.as_str()),
            last_locator: locate(first, fingerprint.locator()),
            last_time: first.time,
            last_end: scope.bounds(first).0,
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
    now: Timestamp,
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
    let elapsed_at = |at: Timestamp| at.duration_since(availability_start).as_nanos();
    let elapsed = elapsed_at(now);

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
        let target = live_start_target(live_start, manifest, bounds, Some(elapsed_at(at)));
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
fn has_started(manifest: &Manifest, period: &Period, now: Timestamp) -> bool {
    let Some(availability_start) = manifest
        .availability_start_time
        .filter(|_| manifest.presentation_type == PresentationType::Dynamic)
    else {
        return true;
    };
    Duration::try_from(now.duration_since(availability_start))
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
        // This planner never reports a reset, so only a lost watcher fails it.
        debug_assert!(matches!(failure, SourceFailure::Lost), "{failure:?}");
        DownloadError::Manifest {
            reason: "MPD watcher stopped without a terminal cause".to_owned(),
        }
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
        scramble_seed: None,
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
        scramble_seed: None,
        meta: SegmentMeta::Dash(Arc::new(meta)),
    }
}

#[cfg(test)]
mod tests;
