//! The per-track DASH planner: MPD snapshots → segment descriptors.
//!
//! One planner runs per selected track inside that track's reactor. It
//! selects the track's Representation and issues available segments under
//! consecutive per-track ordinals (the reactor's `msn`), as the
//! [`SegmentPlanner`] contract requires: DASH segment numbers restart per
//! Period and are absent from time-based addressing, so they cannot be used
//! as sequence numbers.
//!
//! A segment's position within its Representation (its *locator*) is its
//! media time (`$Time$`) under `SegmentTimeline` addressing, where numbers
//! shift when an MPD update drops expired `S` entries, and its number under
//! `@duration` addressing, where the time is derived from the number.
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

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};
use url::Url;

use crate::DownloadError;
use crate::dash::config::{DashSelectionConfig, LiveStartPolicy};
use crate::dash::manifest::{
    ByteRange, Clock, Manifest, ManifestError, Period, PresentationType, RejectedContent,
    Representation, Segment, SegmentWindow, UnsupportedFeature,
};
use crate::dash::selection;
use crate::dash::watcher::MpdSnapshot;
use crate::segment_engine::control::{
    EngineNotice, PlanOutcome, PruneWindow, SegmentPlanner, SourceFailure, TerminalCause,
};
use crate::segment_engine::descriptor::{
    DashSegmentMeta, MediaTrack, SegmentDescriptor, SegmentMeta, SegmentSource,
    inherit_query_params,
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
    /// Presentation end of the newest planned segment, in nanoseconds.
    last_end: i128,
}

/// A segment's stable identity within its Representation (see the module
/// documentation).
fn locator(segment: &Segment, timeline: bool) -> u64 {
    if timeline {
        segment.time
    } else {
        segment.number
    }
}

/// The Period and Representation being planned, with the snapshot they
/// come from.
#[derive(Clone, Copy)]
struct PeriodScope<'a> {
    snapshot: &'a MpdSnapshot,
    period: &'a Period,
    key: &'a PeriodKey,
    representation: &'a Representation,
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
    /// The failure reason of the last failed plan and the unsupported
    /// feature behind it, so it surfaces as a typed error.
    unsupported: Option<(Arc<str>, UnsupportedFeature)>,
    /// Whether a Period lacking this track was reported.
    reported_absence: bool,
}

pub(crate) struct PlannerParams {
    pub track: MediaTrack,
    pub selection: Arc<DashSelectionConfig>,
    pub clock: Arc<dyn Clock>,
    pub identity: SegmentIdentityPolicy,
    pub frontier: Arc<ConsumedFrontier>,
    pub start: StartPosition,
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
            unsupported: None,
            reported_absence: false,
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
                    debug!(track = %self.track, %error, "planning paused before an unusable Period");
                    blocked = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let window = representation.segments(now)?;
            window_len = window_len.max(window.len());
            let scope = PeriodScope {
                snapshot,
                period,
                key: &key,
                representation,
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
    fn reissue(
        &self,
        snapshot: &MpdSnapshot,
        now: DateTime<Utc>,
        outcome: &mut PlanOutcome,
    ) -> Result<(), ManifestError> {
        for epoch in &self.epochs {
            let Some(period) = snapshot
                .manifest
                .periods
                .iter()
                .find(|period| PeriodKey::of(period).same(&epoch.period))
            else {
                continue;
            };
            let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id)
            else {
                continue;
            };
            let scope = PeriodScope {
                snapshot,
                period,
                key: &epoch.period,
                representation,
            };
            let Ok((fingerprint, init_url)) = fingerprint(scope) else {
                continue;
            };
            if fingerprint != epoch.fingerprint {
                // The epoch was superseded; its outstanding work keeps the
                // URLs it was planned with.
                continue;
            }
            outcome.descriptors.push(init_descriptor(
                epoch.init_key.clone(),
                init_url,
                epoch.first_ordinal,
                Arc::clone(&epoch.init_meta),
            ));
            let timeline = epoch.fingerprint.timeline;
            let window = representation.segments(now)?;
            // Each pending segment is looked up on its own: the window
            // between two pending locators can be arbitrarily long.
            for planned in &epoch.planned {
                let index = first_index(&window, |segment| {
                    locator(segment, timeline) >= planned.locator
                })?;
                let Some(segment) = window.get(index).transpose()? else {
                    continue;
                };
                if locator(&segment, timeline) != planned.locator {
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
        let timeline = fingerprint.timeline;
        let newest = self.epochs.back();
        let continuing = newest.filter(|epoch| {
            epoch.period.same(key)
                && *epoch.representation_id == *representation.id
                && epoch.fingerprint == fingerprint
        });
        let first_index = match continuing {
            Some(epoch) => {
                let last = epoch.last_locator;
                first_index(window, |segment| locator(segment, timeline) > last)?
            }
            None => {
                let after = match newest {
                    Some(epoch) if epoch.period.same(key) => Some(epoch.last_end),
                    Some(_) => None,
                    None if self.start.period.same(key) => self.start.after_nanos,
                    None => None,
                };
                match after {
                    Some(after) => first_index(window, |segment| scope.bounds(segment).1 > after)?,
                    None => 0,
                }
            }
        };
        if first_index >= window.len() {
            return Ok(PeriodPlan::Complete);
        }
        if *budget == 0 {
            return Ok(PeriodPlan::Blocked);
        }
        if continuing.is_none() {
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
            let position = locator(&segment, timeline);
            epoch.last_locator = position;
            epoch.last_end = scope.bounds(&segment).1;
            let Some(key) = media_key(&self.identity, scope, epoch, &segment) else {
                warn!(
                    track = %track,
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

    /// The Representation this track records in `period`: the newest
    /// epoch's while it is there, else a fresh selection. Fails with the
    /// typed reason when the Period offers nothing usable for the track
    /// because it was rejected (DRM, WebM, ...).
    fn representation_for<'a>(
        &mut self,
        period: &'a Period,
        key: &PeriodKey,
    ) -> Result<Option<&'a Representation>, ManifestError> {
        if let Some(epoch) = self.epochs.back().filter(|epoch| epoch.period.same(key))
            && let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id)
        {
            return Ok(Some(representation));
        }
        let selected = selection::select(period, self.track, &self.selection);
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
                    track = %self.track,
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
        let init_meta = Arc::new(segment_meta(self.track, scope, first, 0));
        let init = init_descriptor(
            init_key.clone(),
            init_url,
            self.next_ordinal,
            Arc::clone(&init_meta),
        );
        match self.epochs.back() {
            Some(previous) => info!(
                track = %self.track,
                from_period = %previous.period.label(),
                from_representation = %previous.representation_id,
                to_period = %key.label(),
                to_representation = %representation.id,
                "DASH track switches epoch"
            ),
            None => info!(
                track = %self.track,
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
                track = %self.track,
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
            last_locator: locator(first, fingerprint.timeline),
            last_end: scope.bounds(first).0,
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
    let elapsed = (now - availability_start)
        .num_nanoseconds()
        .map(i128::from)
        .ok_or(ManifestError::overflow("live start"))?;

    // The newest Period with an available reference segment that starts at
    // or before the target time.
    for (index, period) in manifest.periods.iter().enumerate().rev() {
        let Some(representation) = reference(period) else {
            continue;
        };
        let window = representation.segments(now)?;
        let Some(last) = window.last() else {
            continue;
        };
        let last = last?;
        let (last_start, last_end) = segment_bounds(&last, representation, period);
        let target = match live_start {
            LiveStartPolicy::LiveEdge => last_start,
            LiveStartPolicy::PresentationDelay(delay) => {
                let delay = match delay.or(manifest.suggested_presentation_delay) {
                    Some(delay) => i128::try_from(delay.as_nanos()).unwrap_or(i128::MAX),
                    None => (last_end - last_start).saturating_mul(3),
                };
                // Never later than the newest segment, which would plan
                // nothing until the clock catches up with the delay.
                elapsed.saturating_sub(delay).min(last_start)
            }
        };
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
        if matches!(
            snapshot.terminal,
            Some(TerminalCause::Failed(_) | TerminalCause::HttpFailure { .. })
        ) {
            return outcome;
        }
        if let Err(error) = self.plan_snapshot(snapshot, &mut outcome) {
            warn!(track = %self.track, %error, "DASH planning failed");
            let reason: Arc<str> = Arc::from(error.to_string());
            if let ManifestError::Unsupported(feature) = error {
                self.unsupported = Some((Arc::clone(&reason), feature));
            }
            return PlanOutcome {
                generation: snapshot.generation,
                terminal: Some(TerminalCause::Failed(reason)),
                ..PlanOutcome::default()
            };
        }
        outcome
    }

    fn failure_error(&self, failure: SourceFailure<'_>) -> DownloadError {
        let reason = match failure {
            SourceFailure::Failed(reason) => {
                if let Some((failed, feature)) = &self.unsupported
                    && failed.as_ref() == reason
                {
                    return ManifestError::Unsupported(feature.clone()).into();
                }
                reason.to_owned()
            }
            SourceFailure::Reset(kind) => format!("{} timeline reset ({kind:?})", self.track),
            SourceFailure::Lost => "MPD watcher stopped without a terminal cause".to_owned(),
        };
        DownloadError::Manifest { reason }
    }
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
        },
        url,
    ))
}

fn segment_meta(
    track: MediaTrack,
    scope: PeriodScope<'_>,
    segment: &Segment,
    duration: u64,
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
        availability_start_time: scope.manifest().availability_start_time,
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
    let meta = segment_meta(track, scope, segment, segment.duration);
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

        fn snapshot(&mut self, mpd: &str) -> MpdSnapshot {
            let url = Url::parse(MPD_URL).unwrap();
            let manifest = Manifest::parse(mpd.as_bytes(), &url).expect("valid MPD");
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
            let snapshot = self.snapshot(mpd);
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
            terminal: Some(TerminalCause::Failed(Arc::from("gone"))),
        };
        let planner = fixture.planner.as_mut().unwrap();
        let outcome = planner.plan(&snapshot);
        assert!(outcome.descriptors.is_empty());
        assert!(matches!(
            planner.failure_error(SourceFailure::Failed("gone")),
            DownloadError::Manifest { .. }
        ));
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
        let Some(TerminalCause::Failed(reason)) = failed.terminal else {
            panic!("expected a failure, got {:?}", failed.terminal);
        };
        let error = fixture
            .planner
            .as_ref()
            .unwrap()
            .failure_error(SourceFailure::Failed(&reason));
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
}
