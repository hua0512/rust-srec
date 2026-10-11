//! The per-track DASH planner: MPD snapshots → segment descriptors.
//!
//! One planner runs per selected track inside that track's reactor. It
//! selects the track's Representation, decides where recording starts, and
//! issues every newly available segment exactly once under consecutive
//! per-track ordinals (the reactor's `msn`), as the [`SegmentPlanner`]
//! contract requires: DASH segment numbers restart per Period and are absent
//! from time-based addressing, so they cannot be used as sequence numbers.
//!
//! A segment's position within its Representation (its *locator*) is its
//! media time (`$Time$`) under `SegmentTimeline` addressing, where numbers
//! shift when an MPD update drops expired `S` entries, and its number under
//! `@duration` addressing, where the time is derived from the number.
//!
//! An *epoch* is one (Period, Representation) pair. Its segments share one
//! initialization segment and one identity scope, so a URL reused by another
//! Period or Representation is a distinct resource.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};
use url::Url;

use crate::DownloadError;
use crate::dash::config::{DashSelectionConfig, LiveStartPolicy};
use crate::dash::manifest::{
    ByteRange, Clock, Manifest, ManifestError, Period, PresentationType, Representation, Segment,
    SegmentWindow,
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

    fn label(&self) -> String {
        match &self.id {
            Some(id) => format!("id:{id}"),
            None => format!("start:{}", self.start.as_nanos()),
        }
    }
}

/// Where planning continues in a Period that has no open epoch yet: every
/// segment ending after `after_nanos` (presentation time on the MPD
/// timeline), or every available segment when `None`.
#[derive(Debug, Clone)]
struct Position {
    period: PeriodKey,
    after_nanos: Option<i128>,
}

/// The open (Period, Representation) pair.
#[derive(Debug)]
struct Epoch {
    period: PeriodKey,
    representation_id: Arc<str>,
    scope: Arc<str>,
    init: SegmentDescriptor,
    /// Whether locators are media times (`SegmentTimeline`) or numbers.
    timeline: bool,
    /// Locator of the newest planned segment.
    last_locator: u64,
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

pub(crate) struct DashPlanner {
    track: MediaTrack,
    selection: Arc<DashSelectionConfig>,
    live_start: LiveStartPolicy,
    clock: Arc<dyn Clock>,
    identity: SegmentIdentityPolicy,
    position: Option<Position>,
    epoch: Option<Epoch>,
    next_ordinal: u64,
}

impl DashPlanner {
    pub(crate) fn new(
        track: MediaTrack,
        selection: Arc<DashSelectionConfig>,
        live_start: LiveStartPolicy,
        clock: Arc<dyn Clock>,
        identity: SegmentIdentityPolicy,
    ) -> Self {
        Self {
            track,
            selection,
            live_start,
            clock,
            identity,
            position: None,
            epoch: None,
            next_ordinal: 0,
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
        if ended && manifest.presentation_type == PresentationType::Dynamic {
            outcome.terminal.get_or_insert(TerminalCause::End);
        }
        if self.position.is_none() {
            match self.choose_start(manifest, now)? {
                Some(position) => {
                    info!(
                        track = %self.track,
                        period = %position.period.label(),
                        "DASH recording start chosen"
                    );
                    self.position = Some(position);
                }
                // Nothing is available for this track yet.
                None => return Ok(()),
            }
        }

        let first_new = self.next_ordinal;
        let mut window_len: u64 = 0;
        let mut target_duration = 0.0_f64;
        for period in &manifest.periods {
            let key = PeriodKey::of(period);
            let Some(position) = &self.position else {
                break;
            };
            if !key.at_or_after(&position.period) {
                continue;
            }
            let Some(representation) = self.representation_for(period, &key) else {
                continue;
            };
            let window = representation.segments(now)?;
            window_len = window.len();
            let scope = PeriodScope {
                snapshot,
                period,
                key: &key,
                representation,
            };
            let planned = self.plan_period(scope, &window, outcome)?;
            if let Some(duration) = planned {
                target_duration = duration;
            }
        }

        if let Some(epoch) = &mut self.epoch {
            // Re-issuing the open epoch's init with the newest ordinal keeps
            // the store from trimming it while media still depends on it.
            let mut init = epoch.init.clone();
            init.msn = self.next_ordinal;
            epoch.init.msn = init.msn;
            outcome.descriptors.push(init);
        }
        let planned = self.next_ordinal - first_new;
        if planned > 0 {
            outcome.notices.push(EngineNotice::ManifestRefreshed {
                sequence_base: first_new,
                target_duration,
            });
        }
        if manifest.presentation_type == PresentationType::Dynamic {
            // Planned segments are never re-issued, so everything finished
            // below the next ordinal may be forgotten. Unfinished segments
            // below it are the download backlog, bounded by the availability
            // window: anything older is gone from the server too.
            outcome.prune = Some(PruneWindow {
                start: self.next_ordinal,
                len: usize::try_from(window_len.max(planned).max(1)).unwrap_or(usize::MAX),
            });
        }
        Ok(())
    }

    /// Plans the newly available segments of one Period. Returns the duration
    /// of the newest segment planned, if any.
    fn plan_period(
        &mut self,
        scope: PeriodScope<'_>,
        window: &SegmentWindow<'_>,
        outcome: &mut PlanOutcome,
    ) -> Result<Option<f64>, ManifestError> {
        let PeriodScope {
            key,
            representation,
            ..
        } = scope;
        let timeline = representation.uses_segment_timeline();
        let continuing = self.epoch.as_ref().filter(|epoch| {
            epoch.period.same(key)
                && *epoch.representation_id == *representation.id
                && epoch.timeline == timeline
        });
        let first_index = match continuing {
            Some(epoch) => {
                let last = epoch.last_locator;
                first_index(window, |segment| locator(segment, timeline) > last)?
            }
            None => match self
                .position
                .as_ref()
                .filter(|position| position.period.same(key))
                .and_then(|position| position.after_nanos)
            {
                Some(after) => first_index(window, |segment| scope.bounds(segment).1 > after)?,
                None => 0,
            },
        };
        if first_index >= window.len() {
            return Ok(None);
        }

        if continuing.is_none() {
            let init = self.open_epoch(scope, window, first_index)?;
            outcome.descriptors.push(init);
        }
        let Some(epoch) = self.epoch.as_mut() else {
            return Ok(None);
        };
        let mut newest = None;
        for segment in window
            .iter()
            .skip(usize::try_from(first_index).unwrap_or(usize::MAX))
        {
            let segment = segment?;
            let Some(descriptor) = media_descriptor(
                self.track,
                &self.identity,
                scope,
                epoch,
                &segment,
                self.next_ordinal,
            ) else {
                warn!(
                    track = %self.track,
                    number = segment.number,
                    "skipping a DASH segment with an invalid byte range"
                );
                continue;
            };
            outcome.descriptors.push(descriptor);
            self.next_ordinal = self
                .next_ordinal
                .checked_add(1)
                .ok_or(ManifestError::overflow("segment ordinal"))?;
            epoch.last_locator = locator(&segment, timeline);
            newest = Some(segment);
        }
        let Some(newest) = newest else {
            return Ok(None);
        };
        self.position = Some(Position {
            period: key.clone(),
            after_nanos: Some(scope.bounds(&newest).1),
        });
        Ok(Some(newest.duration_secs()))
    }

    /// The Representation this track records in `period`: the open epoch's
    /// one while it exists, else a fresh selection. A Representation that
    /// disappears from its Period is replaced, which opens a new epoch and
    /// so a new initialization segment downstream.
    fn representation_for<'a>(
        &self,
        period: &'a Period,
        key: &PeriodKey,
    ) -> Option<&'a Representation> {
        if let Some(epoch) = self.epoch.as_ref().filter(|epoch| epoch.period.same(key)) {
            if let Some(representation) =
                selection::find(period, self.track, &epoch.representation_id)
            {
                return Some(representation);
            }
            let replacement = selection::select(period, self.track, &self.selection);
            debug!(
                track = %self.track,
                representation = %epoch.representation_id,
                replacement = replacement.map(|r| r.id.as_str()),
                "selected Representation left the MPD; reselecting"
            );
            return replacement;
        }
        selection::select(period, self.track, &self.selection)
    }

    fn open_epoch(
        &mut self,
        scope: PeriodScope<'_>,
        window: &SegmentWindow<'_>,
        first_index: u64,
    ) -> Result<SegmentDescriptor, ManifestError> {
        let PeriodScope {
            snapshot,
            key,
            representation,
            ..
        } = scope;
        let first = window
            .get(first_index)
            .ok_or(ManifestError::overflow("segment window"))??;
        let key_scope: Arc<str> = Arc::from(format!(
            "{}\u{1f}{}\u{1f}{}",
            self.track,
            key.label(),
            representation.id
        ));
        let Some(initialization) = &representation.initialization else {
            return Err(ManifestError::inconsistent(
                "selected Representation has no initialization segment",
            ));
        };
        let mut url = initialization.url.clone();
        inherit_query_params(&mut url, &snapshot.parent_params);
        let byte_range = match initialization.byte_range {
            Some(range) => Some(byte_range_key(range).ok_or_else(|| {
                ManifestError::inconsistent("initialization byte range ends before it starts")
            })?),
            None => None,
        };
        let meta = segment_meta(self.track, scope, &first, 0);
        let init = SegmentDescriptor {
            key: SegmentKey {
                kind: SegmentKind::Init,
                uri: self.identity.canonical_uri(&url),
                byte_range,
                scope: Some(Arc::clone(&key_scope)),
            },
            msn: self.next_ordinal,
            source: SegmentSource::Playlist,
            parsed_url: Arc::new(url),
            discontinuity: false,
            encryption: None,
            init_key: None,
            meta: SegmentMeta::Dash(Arc::new(meta)),
        };
        if let Some(previous) = &self.epoch {
            info!(
                track = %self.track,
                from_period = %previous.period.label(),
                from_representation = %previous.representation_id,
                to_period = %key.label(),
                to_representation = %representation.id,
                "DASH track switches epoch"
            );
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
        self.epoch = Some(Epoch {
            period: key.clone(),
            representation_id: Arc::from(representation.id.as_str()),
            scope: key_scope,
            init: init.clone(),
            timeline: representation.uses_segment_timeline(),
            last_locator: locator(&first, representation.uses_segment_timeline()),
        });
        Ok(init)
    }

    /// Where recording starts: the first segment of a static presentation;
    /// for a dynamic one, the segment at the configured distance behind the
    /// live edge. `None` while nothing is available for this track.
    fn choose_start(
        &self,
        manifest: &Manifest,
        now: DateTime<Utc>,
    ) -> Result<Option<Position>, ManifestError> {
        if manifest.presentation_type == PresentationType::Static {
            return Ok(manifest
                .periods
                .iter()
                .find(|period| selection::select(period, self.track, &self.selection).is_some())
                .map(|period| Position {
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

        // The newest Period with an available segment for this track that
        // starts at or before the target time.
        for (index, period) in manifest.periods.iter().enumerate().rev() {
            let Some(representation) = selection::select(period, self.track, &self.selection)
            else {
                continue;
            };
            let window = representation.segments(now)?;
            let Some(last) = window.last() else {
                continue;
            };
            let last = last?;
            let (last_start, last_end) = segment_bounds(&last, representation, period);
            let target = match self.live_start {
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
                return Ok(Some(Position {
                    period: PeriodKey::of(period),
                    after_nanos: Some(target),
                }));
            }
        }
        Ok(None)
    }
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
            return PlanOutcome {
                generation: snapshot.generation,
                terminal: Some(TerminalCause::Failed(Arc::from(error.to_string()))),
                ..PlanOutcome::default()
            };
        }
        outcome
    }

    fn failure_error(&self, failure: SourceFailure<'_>) -> DownloadError {
        let reason = match failure {
            SourceFailure::Failed(reason) => reason.to_owned(),
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
    let start = period_start.saturating_add(offset.saturating_mul(NANOS_PER_SEC) / timescale);
    let end = start
        .saturating_add(i128::from(segment.duration).saturating_mul(NANOS_PER_SEC) / timescale);
    (start, end)
}

fn byte_range_key(range: ByteRange) -> Option<ByteRangeKey> {
    let length = range.last.checked_sub(range.first)?.checked_add(1)?;
    Some(ByteRangeKey {
        length,
        offset: range.first,
    })
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

fn media_descriptor(
    track: MediaTrack,
    identity: &SegmentIdentityPolicy,
    scope: PeriodScope<'_>,
    epoch: &Epoch,
    segment: &Segment,
    ordinal: u64,
) -> Option<SegmentDescriptor> {
    let byte_range = match segment.byte_range {
        Some(range) => Some(byte_range_key(range)?),
        None => None,
    };
    let mut url: Url = segment.url.clone();
    inherit_query_params(&mut url, &scope.snapshot.parent_params);
    let meta = segment_meta(track, scope, segment, segment.duration);
    Some(SegmentDescriptor {
        key: SegmentKey {
            kind: SegmentKind::Media,
            uri: identity.canonical_uri(&url),
            byte_range,
            scope: Some(Arc::clone(&epoch.scope)),
        },
        msn: ordinal,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(url),
        discontinuity: false,
        encryption: None,
        init_key: Some(epoch.init.key.clone()),
        meta: SegmentMeta::Dash(Arc::new(meta)),
    })
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

    struct Fixture {
        clock: Arc<ManualClock>,
        planner: DashPlanner,
        generation: u64,
    }

    impl Fixture {
        fn new(track: MediaTrack, live_start: LiveStartPolicy, elapsed_secs: i64) -> Self {
            let clock = Arc::new(ManualClock::new(
                ast() + chrono::TimeDelta::seconds(elapsed_secs),
            ));
            let planner = DashPlanner::new(
                track,
                Arc::new(DashSelectionConfig::default()),
                live_start,
                Arc::clone(&clock) as Arc<dyn Clock>,
                SegmentIdentityPolicy::default(),
            );
            Self {
                clock,
                planner,
                generation: 0,
            }
        }

        fn plan(&mut self, mpd: &str) -> PlanOutcome {
            let url = Url::parse(MPD_URL).unwrap();
            let manifest = Manifest::parse(mpd.as_bytes(), &url).expect("valid MPD");
            let terminal = (manifest.presentation_type == PresentationType::Static)
                .then_some(TerminalCause::End);
            self.generation += 1;
            let snapshot = MpdSnapshot {
                generation: self.generation,
                manifest_generation: self.generation,
                manifest: Arc::new(manifest),
                document_url: Arc::new(url.clone()),
                parent_params: crate::segment_engine::descriptor::query_params(url.query()).into(),
                fetched_at: self.clock.now(),
                clock_offset: crate::dash::manifest::ClockOffset::ZERO,
                terminal,
            };
            self.planner.plan(&snapshot)
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
        assert_eq!(first.prune.map(|prune| prune.start), Some(3));
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
        assert!(outcome.prune.is_none());

        let video = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0).plan(mpd);
        assert!(media(&video).is_empty(), "no video Representation to plan");
    }

    #[test]
    fn availability_end_time_ends_the_stream_after_the_last_segments() {
        let mpd = number_mpd(r#"availabilityEndTime="2024-01-01T00:00:12Z""#);
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 9);
        let first = fixture.plan(&mpd);
        assert_eq!(media(&first)[0].1.number, 4);
        assert!(first.terminal.is_none());
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
        let outcome = fixture.planner.plan(&snapshot);
        assert!(outcome.descriptors.is_empty());
        assert!(matches!(
            fixture.planner.failure_error(SourceFailure::Failed("gone")),
            DownloadError::Manifest { .. }
        ));
    }
}
