//! Segment timing and availability (ISO/IEC 23009-1 §5.3.9.5, DASH-IF
//! "Restricted timing model" §9, §13).
//!
//! All arithmetic is exact: wall-clock and presentation times are integer
//! nanoseconds, and comparisons against media times multiply through by the
//! timescale instead of dividing, so boundaries are decided without rounding.
//!
//! Notation used below: `P` Period start, `T` timescale, `PTO`
//! presentationTimeOffset, `A` availabilityTimeOffset, `B`
//! timeShiftBufferDepth, `x = now − availabilityStartTime`.
//!
//! - Media time `m` maps to presentation time `P + (m − PTO) / T` (DASH-IF
//!   timing model §9.1).
//! - `@duration` addressing is a single open run of segments starting at media
//!   time `PTO`, so segment `n` covers `[P + (n−N)·D, P + (n−N+1)·D)` with
//!   `D = @duration / T`, anchored to the Period start.
//! - A segment of a dynamic presentation is available once its end satisfies
//!   `end ≤ x + A`, and stays available while `end ≥ x − B` (DASH-IF timing
//!   model §13.3: a segment is available when its end point is inside or at
//!   the end of the availability window `[now − B, now + A]`). Both boundaries
//!   are inclusive. Without `B` the window starts at the Period start.
//! - Segments are clamped to the Period: a segment must end after the Period
//!   start and start before the Period end.
//! - After `MPD@availabilityEndTime` no segment is available.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use url::Url;

use super::error::{ManifestError, UnsupportedFeature};
use super::parse::{RawS, RawSegmentUrl};
use super::resolve::PresentationType;
use super::template::Template;
use super::xs;

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// `@availabilityTimeOffset`: how much earlier than its nominal availability
/// a segment can be requested (ISO/IEC 23009-1 §5.3.9.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityTimeOffset {
    Finite(Duration),
    /// `INF`: every segment of the Period is available as long as the MPD is
    /// valid (DASH-IF IOP v4.2).
    Infinite,
}

impl AvailabilityTimeOffset {
    pub(crate) const ZERO: Self = Self::Finite(Duration::ZERO);

    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Finite(a), Self::Finite(b)) => a.checked_add(b).map(Self::Finite),
            _ => Some(Self::Infinite),
        }
    }
}

/// An inclusive byte range (`first-last`) within a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteRange {
    pub first: u64,
    pub last: u64,
}

/// An initialization segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitSegment {
    pub url: Url,
    pub byte_range: Option<ByteRange>,
}

/// How a Representation's segments are addressed.
///
/// This determines a segment's stable identity across MPD updates: what its
/// URL is built from. That is [`Segment::number`] for `$Number$` templates
/// (with or without a `SegmentTimeline`) and for any `@duration`
/// addressing, and [`Segment::time`] for every other `SegmentTimeline`
/// addressing: `$Time$` templates, timed `SegmentList`s and indexed
/// subsegments (see [`Segment`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressingKind {
    /// `SegmentTemplate@media` with `$Number$`.
    Number,
    /// `SegmentTemplate@media` with `$Time$`.
    Time,
    /// `SegmentList`.
    List,
    /// `SegmentBase`: subsegments of one resource, located by its Segment
    /// Index.
    Indexed,
}

/// A media segment.
///
/// Identity across MPD updates: under `SegmentTimeline` addressing it is
/// [`Self::time`] unless the URL is a `$Number$` template, because numbers
/// are positions in the listed timeline and shift when expired `S` entries
/// are removed. A `$Number$` URL pins the number to the resource, so there
/// the number is the identity, and the times are what an update may
/// rewrite. Under `@duration` addressing (`$Number$` templates and lists) it
/// is [`Self::number`], from which the time is derived.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Segment number (`$Number$`): `startNumber` + position in the
    /// Representation's segment sequence.
    pub number: u64,
    /// Earliest presentation time in media timescale units (`$Time$`); the
    /// presentationTimeOffset is not subtracted.
    pub time: u64,
    /// Duration in media timescale units. The last repetition of an `S@r=-1`
    /// entry that would overlap the next `S@t` is shortened to end there.
    pub duration: u64,
    pub timescale: u64,
    /// Start on the MPD timeline in seconds (relative to
    /// `availabilityStartTime` for dynamic presentations):
    /// `P + (time − PTO) / timescale`. For display and logging only; use the
    /// integer fields for arithmetic.
    pub start_secs: f64,
    pub url: Url,
    pub byte_range: Option<ByteRange>,
}

impl Segment {
    /// Duration in seconds, for display and logging.
    pub fn duration_secs(&self) -> f64 {
        self.duration as f64 / self.timescale as f64
    }
}

/// How far ahead of the availability window a running Period's
/// `SegmentTimeline` lies when none of its listed segments is available
/// (see [`super::Representation::timeline_ahead`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineLead {
    /// Time until the earliest listed segment becomes available.
    pub earliest: Duration,
    /// Time until the newest listed segment becomes available, at most
    /// until `availabilityEndTime`.
    pub newest: Duration,
    /// The earliest listed segment's duration.
    pub segment_duration: Duration,
}

/// A run of equal-duration segments: one `S` element, or the whole
/// `@duration` sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    /// Media time of the first segment.
    pub start: u64,
    pub duration: u64,
    /// `None` for an open run (`@duration`, or a trailing `S@r=-1`), which is
    /// bounded by the Period end or the availability window.
    pub count: Option<u64>,
    /// Position of the first segment in the Representation's sequence.
    pub first_index: u64,
}

/// Where segment URLs come from.
#[derive(Debug, Clone)]
pub(crate) enum SegmentUrls {
    Template {
        media: Template,
        base: Url,
    },
    /// `SegmentURL`s are shared with every Representation inheriting the
    /// list and resolved against `base` when a segment is produced.
    List {
        urls: Arc<[RawSegmentUrl]>,
        base: Url,
    },
    /// Byte ranges of the subsegments of `url`, one per segment, from its
    /// Segment Index.
    Indexed {
        url: Url,
        ranges: Arc<[ByteRange]>,
    },
}

/// Resolved segment addressing of a Representation.
#[derive(Debug, Clone)]
pub(crate) struct Addressing {
    pub timescale: u64,
    pub presentation_time_offset: u64,
    pub start_number: u64,
    pub end_number: Option<u64>,
    /// Whether the runs come from a `SegmentTimeline` (else `@duration`).
    pub has_timeline: bool,
    /// Shared by every Representation inheriting the same timeline.
    pub runs: Arc<[Run]>,
    pub urls: SegmentUrls,
}

/// The parts of the MPD and Period that availability depends on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeriodTiming {
    pub presentation_type: PresentationType,
    /// `MPD@availabilityStartTime` in nanoseconds since the Unix epoch.
    pub availability_start: Option<i128>,
    /// Period start on the MPD timeline, in nanoseconds.
    pub period_start: i128,
    /// Period duration in nanoseconds; `None` while open.
    pub period_duration: Option<i128>,
    pub time_shift_buffer_depth: Option<i128>,
    /// `MPD@availabilityEndTime` in nanoseconds since the Unix epoch.
    pub availability_end: Option<i128>,
}

impl Addressing {
    /// Builds the runs of a `SegmentTimeline` (ISO/IEC 23009-1 §5.3.9.6).
    ///
    /// `S@t` omitted takes the previous segment's end, or 0 for the first `S`.
    /// `S@r=-1` repeats until the next `S@t` or, on the last `S`, until the
    /// Period end or the availability window, which are only known at
    /// evaluation time. Before a next `S@t`, the repetition count rounds up as
    /// the reference players do, and a last repetition that would overlap the
    /// next `S@t` is shortened to end there, so segment ends stay increasing.
    /// Each run is stored arithmetically; repetitions are never expanded.
    pub(crate) fn timeline_runs(entries: &[RawS]) -> Result<Vec<Run>, ManifestError> {
        let overflow = || ManifestError::overflow("SegmentTimeline");
        let mut runs = Vec::with_capacity(entries.len());
        let mut previous_end: Option<u64> = None;
        let mut first_index: u64 = 0;
        let mut push = |runs: &mut Vec<Run>, start: u64, duration: u64, count: Option<u64>| {
            runs.push(Run {
                start,
                duration,
                count,
                first_index,
            });
            if let Some(count) = count {
                first_index = first_index.checked_add(count).ok_or_else(overflow)?;
            }
            Ok::<(), ManifestError>(())
        };
        for (position, entry) in entries.iter().enumerate() {
            let start = match (entry.t, previous_end) {
                (Some(t), Some(end)) if t < end => {
                    return Err(ManifestError::timeline(format!(
                        "S[{position}]@t={t} starts before the previous segment ends at {end}"
                    )));
                }
                (Some(t), _) => t,
                (None, end) => end.unwrap_or(0),
            };
            if entry.r >= 0 {
                let repeats = u64::try_from(entry.r).map_err(|_| overflow())?;
                let count = repeats.checked_add(1).ok_or_else(overflow)?;
                let length = count.checked_mul(entry.d).ok_or_else(overflow)?;
                previous_end = Some(start.checked_add(length).ok_or_else(overflow)?);
                push(&mut runs, start, entry.d, Some(count))?;
                continue;
            }
            let Some(next) = entries.get(position + 1) else {
                push(&mut runs, start, entry.d, None)?;
                previous_end = None;
                continue;
            };
            let next_t = next.t.ok_or_else(|| {
                ManifestError::timeline(format!(
                    "S[{position}]@r=-1 must be last or followed by an S with @t"
                ))
            })?;
            let span = next_t.checked_sub(start).ok_or_else(|| {
                ManifestError::timeline(format!(
                    "S[{}]@t={next_t} is before S[{position}]@t={start}",
                    position + 1
                ))
            })?;
            let full = span / entry.d;
            let remainder = span % entry.d;
            if full > 0 {
                push(&mut runs, start, entry.d, Some(full))?;
            }
            if remainder > 0 {
                push(&mut runs, next_t - remainder, remainder, Some(1))?;
            }
            previous_end = Some(next_t);
        }
        Ok(runs)
    }

    /// The range of sequence positions (`number − startNumber`) available at
    /// `now`, or the whole Period for static presentations.
    pub(crate) fn window(
        &self,
        timing: &PeriodTiming,
        offset: AvailabilityTimeOffset,
        now: DateTime<Utc>,
    ) -> Result<Range<u64>, ManifestError> {
        let overflow = || ManifestError::overflow("segment availability");
        let timescale = i128::from(self.timescale);
        let pto = i128::from(self.presentation_time_offset);

        if let Some(end) = timing.availability_end
            && xs::instant_nanos(now) > end
        {
            return Ok(0..0);
        }
        if self
            .end_number
            .is_some_and(|end_number| end_number < self.start_number)
        {
            return Err(ManifestError::inconsistent(
                "@endNumber is below @startNumber",
            ));
        }
        // Positions available at all: up to `@endNumber`, and one per
        // `SegmentURL` or indexed subsegment. `@endNumber` is the number of
        // the last segment (inclusive); `u64::MAX − 0 + 1` overflows only
        // when nothing is capped.
        let mut cap = self
            .end_number
            .and_then(|end_number| (end_number - self.start_number).checked_add(1));
        let listed = match &self.urls {
            SegmentUrls::Template { .. } => None,
            SegmentUrls::List { urls, .. } => Some(urls.len()),
            SegmentUrls::Indexed { ranges, .. } => Some(ranges.len()),
        };
        if let Some(listed) = listed {
            let len = u64::try_from(listed).map_err(|_| overflow())?;
            cap = Some(cap.map_or(len, |cap| cap.min(len)));
        }

        // Bounds in "scaled" units (nanoseconds × timescale) relative to the
        // Period start, so that a media time m compares as (m − PTO) · 1e9.
        let period_end = timing
            .period_duration
            .map(|duration| duration.checked_mul(timescale).ok_or_else(overflow))
            .transpose()?;
        let (upper, lower) = match timing.presentation_type {
            PresentationType::Static => (None, None),
            PresentationType::Dynamic => {
                let ast = timing
                    .availability_start
                    .ok_or(ManifestError::MissingAttribute {
                        element: "MPD",
                        attribute: "availabilityStartTime",
                    })?;
                let elapsed = xs::instant_nanos(now)
                    .checked_sub(ast)
                    .and_then(|x| x.checked_sub(timing.period_start))
                    .ok_or_else(overflow)?;
                let upper = match offset {
                    AvailabilityTimeOffset::Finite(offset) => Some(
                        elapsed
                            .checked_add(xs::duration_nanos(offset))
                            .and_then(|bound| bound.checked_mul(timescale))
                            .ok_or_else(overflow)?,
                    ),
                    AvailabilityTimeOffset::Infinite => None,
                };
                let lower = timing
                    .time_shift_buffer_depth
                    .map(|depth| {
                        elapsed
                            .checked_sub(depth)
                            .and_then(|bound| bound.checked_mul(timescale))
                            .ok_or_else(overflow)
                    })
                    .transpose()?;
                (upper, lower)
            }
        };
        let finite_runs = self.runs.iter().all(|run| run.count.is_some());
        if upper.is_none() && period_end.is_none() && cap.is_none() && !finite_runs {
            return Err(match timing.presentation_type {
                PresentationType::Static => ManifestError::inconsistent(
                    "static presentation whose last Period has no duration",
                ),
                PresentationType::Dynamic => UnsupportedFeature::UnboundedAvailability.into(),
            });
        }

        let mut first: Option<u64> = None;
        let mut end: u64 = 0;
        for run in self.runs.iter() {
            let duration = i128::from(run.duration);
            let step = duration.checked_mul(NANOS_PER_SEC).ok_or_else(overflow)?;
            // Scaled offset of the run's first segment start from the Period start.
            let origin = (i128::from(run.start) - pto)
                .checked_mul(NANOS_PER_SEC)
                .ok_or_else(overflow)?;

            // Ends after the Period start: start + (i+1)·d > PTO.
            let mut low = floor_div(pto - i128::from(run.start), duration).max(0);
            let mut high = match run.count {
                Some(count) => i128::from(count) - 1,
                None => i128::MAX,
            };
            if let Some(cap) = cap {
                high = high.min(i128::from(cap) - i128::from(run.first_index) - 1);
            }
            // Starts before the Period end: origin + i·step < end.
            if let Some(period_end) = period_end {
                let room = period_end.checked_sub(origin).ok_or_else(overflow)?;
                high = high.min(ceil_div(room, step).ok_or_else(overflow)? - 1);
            }
            // Available: origin + (i+1)·step ≤ upper.
            if let Some(upper) = upper {
                let room = upper.checked_sub(origin).ok_or_else(overflow)?;
                high = high.min(floor_div(room, step) - 1);
            }
            // Not expired: origin + (i+1)·step ≥ lower.
            if let Some(lower) = lower {
                let room = lower.checked_sub(origin).ok_or_else(overflow)?;
                low = low.max(ceil_div(room, step).ok_or_else(overflow)? - 1);
            }
            if low > high {
                continue;
            }
            let base = i128::from(run.first_index);
            let position = |offset: i128| {
                base.checked_add(offset)
                    .and_then(|position| u64::try_from(position).ok())
                    .ok_or_else(overflow)
            };
            let run_first = position(low)?;
            let run_end = position(high.checked_add(1).ok_or_else(overflow)?)?;
            first.get_or_insert(run_first);
            end = end.max(run_end);
        }

        let Some(first) = first else {
            return Ok(0..0);
        };
        Ok(first..end)
    }

    /// How far ahead of the availability window at `now` the segments the
    /// `SegmentTimeline` lists lie, when not one of them is available yet
    /// and the Period has begun and not ended by the clock. `None`
    /// otherwise, and without a timeline, for static presentations or with
    /// an infinite availability time offset.
    ///
    /// Listed means described by an `S` entry and allowed by `@endNumber`,
    /// the Period's bounds and `availabilityEndTime`; a trailing `S@r=-1`
    /// lists only its first segment, since later repetitions are
    /// extrapolated.
    pub(crate) fn timeline_ahead(
        &self,
        timing: &PeriodTiming,
        offset: AvailabilityTimeOffset,
        now: DateTime<Utc>,
    ) -> Result<Option<TimelineLead>, ManifestError> {
        let overflow = || ManifestError::overflow("timeline lead");
        if !self.has_timeline || timing.presentation_type == PresentationType::Static {
            return Ok(None);
        }
        let AvailabilityTimeOffset::Finite(offset) = offset else {
            return Ok(None);
        };
        let Some(ast) = timing.availability_start else {
            return Err(ManifestError::MissingAttribute {
                element: "MPD",
                attribute: "availabilityStartTime",
            });
        };
        let elapsed = xs::instant_nanos(now)
            .checked_sub(ast)
            .ok_or_else(overflow)?;
        let into_period = elapsed
            .checked_sub(timing.period_start)
            .ok_or_else(overflow)?;
        if into_period < 0
            || timing
                .period_duration
                .is_some_and(|duration| into_period >= duration)
        {
            return Ok(None);
        }

        // The listed positions: the window without its clock bounds, with
        // an open trailing run closed after its first segment.
        let closed;
        let listing = if self.runs.last().is_some_and(|run| run.count.is_none()) {
            let runs: Vec<Run> = self
                .runs
                .iter()
                .map(|run| Run {
                    count: Some(run.count.unwrap_or(1)),
                    ..*run
                })
                .collect();
            closed = Addressing {
                runs: runs.into(),
                ..self.clone()
            };
            &closed
        } else {
            self
        };
        let unclocked = PeriodTiming {
            presentation_type: PresentationType::Static,
            ..*timing
        };
        let listed = listing.window(&unclocked, AvailabilityTimeOffset::ZERO, now)?;
        let (Some(first), Some(last)) = (
            listing.run_timing(listed.start),
            listed
                .end
                .checked_sub(1)
                .and_then(|last| listing.run_timing(last)),
        ) else {
            return Ok(None);
        };

        let timescale = i128::from(self.timescale.max(1));
        let pto = i128::from(self.presentation_time_offset);
        // Rounded up so that evaluating availability that much later makes
        // the segment available exactly: the window compares unrounded ticks.
        let end = |(time, duration): (u64, u64)| {
            (i128::from(time) + i128::from(duration) - pto)
                .checked_mul(NANOS_PER_SEC)
                .and_then(|scaled| ceil_div(scaled, timescale))
                .and_then(|end| end.checked_add(timing.period_start))
                .ok_or_else(overflow)
        };
        let edge = elapsed
            .checked_add(xs::duration_nanos(offset))
            .ok_or_else(overflow)?;
        let earliest = end(first)? - edge;
        if earliest <= 0 {
            return Ok(None);
        }
        let mut newest = end(last)? - edge;
        // Nothing is available after `availabilityEndTime`, so the newest
        // listed segment that can still become available is the one
        // available by then.
        if let Some(availability_end) = timing.availability_end {
            let remaining = availability_end
                .checked_sub(xs::instant_nanos(now))
                .ok_or_else(overflow)?;
            if remaining < earliest {
                return Ok(None);
            }
            newest = newest.min(remaining);
        }
        let nanos = |value: i128| Duration::from_nanos(u64::try_from(value).unwrap_or(u64::MAX));
        Ok(Some(TimelineLead {
            earliest: nanos(earliest),
            newest: nanos(newest),
            segment_duration: nanos(i128::from(first.1).saturating_mul(NANOS_PER_SEC) / timescale),
        }))
    }

    /// Media time and duration of the segment at sequence position
    /// `index`, when a run covers it.
    fn run_timing(&self, index: u64) -> Option<(u64, u64)> {
        let position = self.runs.partition_point(|run| run.first_index <= index);
        let run = self.runs.get(position.checked_sub(1)?)?;
        let repetition = index - run.first_index;
        if run.count.is_some_and(|count| repetition >= count) {
            return None;
        }
        let time = run
            .start
            .checked_add(repetition.checked_mul(run.duration)?)?;
        Some((time, run.duration))
    }

    /// The window available at `now`, with its segment numbers validated.
    pub(crate) fn segments(
        &self,
        timing: &PeriodTiming,
        offset: AvailabilityTimeOffset,
        now: DateTime<Utc>,
    ) -> Result<SegmentWindow<'_>, ManifestError> {
        let positions = self.window(timing, offset, now)?;
        SegmentWindow::new(self, timing.period_start, positions)
    }

    pub(crate) fn kind(&self) -> AddressingKind {
        match &self.urls {
            SegmentUrls::Template { media, .. } if media.uses_time() => AddressingKind::Time,
            SegmentUrls::Template { .. } => AddressingKind::Number,
            SegmentUrls::List { .. } => AddressingKind::List,
            SegmentUrls::Indexed { .. } => AddressingKind::Indexed,
        }
    }

    /// The segment at sequence position `index`, which must be covered by a
    /// run (see [`Self::window`]).
    pub(crate) fn segment(&self, index: u64, period_start: i128) -> Result<Segment, ManifestError> {
        let overflow = || ManifestError::overflow("segment position");
        let position = self.runs.partition_point(|run| run.first_index <= index);
        let run = position
            .checked_sub(1)
            .and_then(|position| self.runs.get(position))
            .ok_or_else(overflow)?;
        let repetition = index - run.first_index;
        if run.count.is_some_and(|count| repetition >= count) {
            return Err(overflow());
        }
        let time = repetition
            .checked_mul(run.duration)
            .and_then(|offset| run.start.checked_add(offset))
            .ok_or_else(overflow)?;
        let number = self.start_number.checked_add(index).ok_or_else(overflow)?;
        let (url, byte_range) = match &self.urls {
            SegmentUrls::Template { media, base } => {
                let url = base.join(&media.expand(number, time)).map_err(|source| {
                    ManifestError::InvalidUrl {
                        element: "SegmentTemplate@media",
                        source,
                    }
                })?;
                (url, None)
            }
            SegmentUrls::List { urls, base } => {
                let entry = usize::try_from(index)
                    .ok()
                    .and_then(|index| urls.get(index))
                    .ok_or_else(overflow)?;
                // Without `@media` the BaseURL itself is the resource.
                let url = match &entry.media {
                    Some(media) => {
                        base.join(media)
                            .map_err(|source| ManifestError::InvalidUrl {
                                element: "SegmentURL@media",
                                source,
                            })?
                    }
                    None => base.clone(),
                };
                (url, entry.media_range)
            }
            SegmentUrls::Indexed { url, ranges } => {
                let range = usize::try_from(index)
                    .ok()
                    .and_then(|index| ranges.get(index))
                    .ok_or_else(overflow)?;
                (url.clone(), Some(*range))
            }
        };
        let offset_ticks = i128::from(time) - i128::from(self.presentation_time_offset);
        let start_secs = period_start as f64 / NANOS_PER_SEC as f64
            + offset_ticks as f64 / self.timescale as f64;
        Ok(Segment {
            number,
            time,
            duration: run.duration,
            timescale: self.timescale,
            start_secs,
            url,
            byte_range,
        })
    }
}

/// `⌊a / b⌋` for `b > 0`.
fn floor_div(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

/// `⌈a / b⌉` for `b > 0`; `None` only for `a = i128::MIN`.
fn ceil_div(a: i128, b: i128) -> Option<i128> {
    Some(-(a.checked_neg()?.div_euclid(b)))
}

/// The segments of a Representation that are available at a given time.
///
/// Segments are produced on demand, so a window over years of `@duration`
/// segments costs nothing until it is iterated.
#[derive(Debug, Clone)]
pub struct SegmentWindow<'a> {
    addressing: &'a Addressing,
    period_start: i128,
    positions: Range<u64>,
    numbers: Range<u64>,
}

impl<'a> SegmentWindow<'a> {
    /// Fails when a segment number in the window would exceed `u64`.
    pub(crate) fn new(
        addressing: &'a Addressing,
        period_start: i128,
        positions: Range<u64>,
    ) -> Result<Self, ManifestError> {
        let number = |position: u64| {
            addressing
                .start_number
                .checked_add(position)
                .ok_or(ManifestError::overflow("segment number"))
        };
        let numbers = number(positions.start)?..number(positions.end)?;
        Ok(Self {
            addressing,
            period_start,
            positions,
            numbers,
        })
    }

    pub fn len(&self) -> u64 {
        self.positions.end - self.positions.start
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Segment numbers in the window (`$Number$` values), half-open; its
    /// length equals [`Self::len`].
    pub fn numbers(&self) -> Range<u64> {
        self.numbers.clone()
    }

    /// The `offset`-th segment of the window (0 is the oldest).
    pub fn get(&self, offset: u64) -> Option<Result<Segment, ManifestError>> {
        let index = self.positions.start.checked_add(offset)?;
        self.positions
            .contains(&index)
            .then(|| self.addressing.segment(index, self.period_start))
    }

    pub fn first(&self) -> Option<Result<Segment, ManifestError>> {
        self.get(0)
    }

    pub fn last(&self) -> Option<Result<Segment, ManifestError>> {
        self.len()
            .checked_sub(1)
            .and_then(|offset| self.get(offset))
    }

    /// Iterates from the oldest segment. `nth`, `nth_back`, `last`, `skip` and
    /// `rev` are O(1) per step, so the newest segments of a long window are
    /// cheap to reach.
    pub fn iter(&self) -> SegmentIter<'a> {
        SegmentIter {
            addressing: self.addressing,
            period_start: self.period_start,
            positions: self.positions.clone(),
        }
    }
}

impl<'a> IntoIterator for &SegmentWindow<'a> {
    type Item = Result<Segment, ManifestError>;
    type IntoIter = SegmentIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over a [`SegmentWindow`].
#[derive(Debug, Clone)]
pub struct SegmentIter<'a> {
    addressing: &'a Addressing,
    period_start: i128,
    positions: Range<u64>,
}

impl Iterator for SegmentIter<'_> {
    type Item = Result<Segment, ManifestError>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.positions.next()?;
        Some(self.addressing.segment(index, self.period_start))
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let index = self.positions.nth(n)?;
        Some(self.addressing.segment(index, self.period_start))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.positions.size_hint()
    }

    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }

    fn count(self) -> usize {
        // Saturates only where `usize` is narrower than `u64`.
        usize::try_from(self.positions.end - self.positions.start).unwrap_or(usize::MAX)
    }
}

impl DoubleEndedIterator for SegmentIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let index = self.positions.next_back()?;
        Some(self.addressing.segment(index, self.period_start))
    }

    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let index = self.positions.nth_back(n)?;
        Some(self.addressing.segment(index, self.period_start))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::super::template::TemplateKind;
    use super::*;

    const SEC: i128 = NANOS_PER_SEC;

    fn s(t: Option<u64>, d: u64, r: i64) -> RawS {
        RawS {
            t,
            d,
            r,
            has_n: false,
            k: None,
        }
    }

    fn addressing(runs: Vec<Run>, timescale: u64, pto: u64, media: &str) -> Addressing {
        Addressing {
            timescale,
            presentation_time_offset: pto,
            start_number: 1,
            end_number: None,
            has_timeline: false,
            runs: runs.into(),
            urls: SegmentUrls::Template {
                media: Template::compile(media, TemplateKind::Media, "v", 1).unwrap(),
                base: Url::parse("https://cdn.example.com/live/").unwrap(),
            },
        }
    }

    fn grid(duration: u64, timescale: u64, pto: u64) -> Addressing {
        addressing(
            vec![Run {
                start: pto,
                duration,
                count: None,
                first_index: 0,
            }],
            timescale,
            pto,
            "$Number$.m4s",
        )
    }

    fn dynamic(period_start: i128, depth: Option<i128>) -> PeriodTiming {
        PeriodTiming {
            presentation_type: PresentationType::Dynamic,
            availability_start: Some(0),
            period_start,
            period_duration: None,
            time_shift_buffer_depth: depth,
            availability_end: None,
        }
    }

    fn at(secs: i64, nanos: u32) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, nanos).unwrap()
    }

    fn finite(secs: u64) -> AvailabilityTimeOffset {
        AvailabilityTimeOffset::Finite(Duration::from_secs(secs))
    }

    #[test]
    fn duration_grid_upper_bound_is_inclusive() {
        // 2 s segments, Period at 10 s: segment n (from 1) ends at 10 + 2n.
        let addressing = grid(2, 1, 0);
        let timing = dynamic(10 * SEC, None);
        let window = |secs, nanos| {
            addressing
                .window(&timing, finite(0), at(secs, nanos))
                .unwrap()
        };
        assert_eq!(window(11, 999_999_999), 0..0);
        assert_eq!(
            window(12, 0),
            0..1,
            "a segment is available exactly at its end"
        );
        assert_eq!(window(13, 999_999_999), 0..1);
        assert_eq!(window(14, 0), 0..2);
        assert_eq!(window(5, 0), 0..0, "before the Period starts");
    }

    #[test]
    fn duration_grid_n_max_matches_closed_form() {
        // n_max = N + floor((x + A − P) / D) − 1 with N=1, D=2.002 (T=1000).
        let addressing = grid(2002, 1000, 0);
        let timing = dynamic(0, None);
        for x in [2.002f64, 4.003, 100.0, 1000.5] {
            let now = at(x.trunc() as i64, ((x.fract() * 1e9).round()) as u32);
            let window = addressing.window(&timing, finite(1), now).unwrap();
            let expected = ((x + 1.0) / 2.002).floor() as u64;
            assert_eq!(window.end, expected, "x={x}");
        }
    }

    #[test]
    fn time_shift_buffer_lower_bound_is_inclusive() {
        let addressing = grid(2, 1, 0);
        let timing = dynamic(0, Some(6 * SEC));
        // x = 20: available ends 2..=20, lower bound end ≥ 14 → ends 14..=20.
        let window = addressing.window(&timing, finite(0), at(20, 0)).unwrap();
        assert_eq!(window, 6..10);
        // Just after: the segment ending at 14 has expired.
        let window = addressing.window(&timing, finite(0), at(20, 1)).unwrap();
        assert_eq!(window, 7..10);
    }

    #[test]
    fn missing_time_shift_buffer_keeps_the_whole_period() {
        let addressing = grid(2, 1, 0);
        let window = addressing
            .window(&dynamic(0, None), finite(0), at(20, 0))
            .unwrap();
        assert_eq!(window, 0..10);
    }

    #[test]
    fn zero_time_shift_buffer_retains_the_live_edge_segment_only_at_exact_boundaries() {
        let addressing = grid(2, 1, 0);
        let window = addressing
            .window(&dynamic(0, Some(0)), finite(0), at(21, 0))
            .unwrap();
        assert!(
            window.is_empty(),
            "the segment ending at 20 expired after 20"
        );
        let window = addressing
            .window(&dynamic(0, Some(0)), finite(0), at(20, 0))
            .unwrap();
        assert_eq!(window, 9..10, "a segment ending exactly at now is kept");
    }

    #[test]
    fn availability_time_offset_moves_the_live_edge() {
        let addressing = grid(2, 1, 0);
        let timing = dynamic(0, None);
        let window = addressing.window(&timing, finite(1), at(11, 0)).unwrap();
        assert_eq!(window, 0..6, "the segment ending at 12 is available at 11");
    }

    #[test]
    fn grid_ignores_presentation_time_offset_for_timing() {
        // PTO only shifts media times ($Time$); the grid is anchored at P.
        let addressing = grid(2, 1, 1_000);
        let timing = dynamic(0, None);
        let window = addressing.window(&timing, finite(0), at(4, 0)).unwrap();
        assert_eq!(window, 0..2);
        let segment = addressing.segment(1, 0).unwrap();
        assert_eq!(segment.time, 1_002);
        assert_eq!(segment.start_secs, 2.0);
    }

    #[test]
    fn period_end_clamps_and_includes_the_overlapping_segment() {
        let addressing = grid(4, 1, 0);
        let timing = PeriodTiming {
            period_duration: Some(10 * SEC),
            ..dynamic(0, None)
        };
        // Segments start at 0, 4, 8; the third overlaps the end at 10.
        let window = addressing.window(&timing, finite(0), at(1_000, 0)).unwrap();
        assert_eq!(window, 0..3);
    }

    #[test]
    fn static_presentation_lists_the_whole_period() {
        let addressing = grid(2, 1, 0);
        let timing = PeriodTiming {
            presentation_type: PresentationType::Static,
            availability_start: None,
            period_start: 0,
            period_duration: Some(9 * SEC),
            time_shift_buffer_depth: None,
            availability_end: None,
        };
        let window = addressing.window(&timing, finite(0), at(0, 0)).unwrap();
        assert_eq!(window, 0..5);
        let open = PeriodTiming {
            period_duration: None,
            ..timing
        };
        assert!(addressing.window(&open, finite(0), at(0, 0)).is_err());
    }

    #[test]
    fn infinite_offset_needs_a_period_end() {
        let addressing = grid(2, 1, 0);
        let timing = dynamic(0, None);
        assert!(matches!(
            addressing.window(&timing, AvailabilityTimeOffset::Infinite, at(0, 0)),
            Err(ManifestError::Unsupported(
                UnsupportedFeature::UnboundedAvailability
            ))
        ));
        let closed = PeriodTiming {
            period_duration: Some(10 * SEC),
            ..timing
        };
        let window = addressing
            .window(&closed, AvailabilityTimeOffset::Infinite, at(0, 0))
            .unwrap();
        assert_eq!(window, 0..5);
    }

    #[test]
    fn timeline_with_omitted_t_and_repeats() {
        let runs = Addressing::timeline_runs(&[s(None, 10, 2), s(None, 5, 0), s(Some(100), 10, 1)])
            .unwrap();
        let starts: Vec<_> = runs
            .iter()
            .map(|run| (run.start, run.count, run.first_index))
            .collect();
        assert_eq!(
            starts,
            [(0, Some(3), 0), (30, Some(1), 3), (100, Some(2), 4)]
        );
    }

    #[test]
    fn timeline_r_minus_one_is_bounded_by_next_t() {
        let runs = Addressing::timeline_runs(&[s(Some(0), 10, -1), s(Some(35), 5, 0)]).unwrap();
        // Rounds up like the reference players (4 repetitions), with the
        // fourth cut to end at the next `S@t`.
        let layout: Vec<_> = runs
            .iter()
            .map(|run| (run.start, run.duration, run.count, run.first_index))
            .collect();
        assert_eq!(
            layout,
            [
                (0, 10, Some(3), 0),
                (30, 5, Some(1), 3),
                (35, 5, Some(1), 4)
            ]
        );
        // An exact multiple needs no shortened repetition.
        let runs = Addressing::timeline_runs(&[s(Some(0), 10, -1), s(Some(40), 5, 0)]).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].count, Some(4));
        // A next `S@t` equal to the start leaves no repetition.
        let runs = Addressing::timeline_runs(&[s(Some(0), 10, -1), s(Some(0), 5, 0)]).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first_index, 0);
    }

    #[test]
    fn timeline_rejects_invalid_layouts() {
        assert!(Addressing::timeline_runs(&[s(Some(0), 10, 1), s(Some(15), 10, 0)]).is_err());
        assert!(Addressing::timeline_runs(&[s(Some(0), 10, -1), s(None, 10, 0)]).is_err());
        assert!(Addressing::timeline_runs(&[s(Some(50), 10, -1), s(Some(40), 10, 0)]).is_err());
        assert!(Addressing::timeline_runs(&[s(Some(u64::MAX - 5), 10, 0), s(None, 1, 0)]).is_err());
        assert!(Addressing::timeline_runs(&[s(Some(0), u64::MAX, i64::MAX)]).is_err());
    }

    #[test]
    fn open_timeline_is_bounded_by_availability() {
        // $Time$ live: 90 kHz, PTO 0, one open S starting at 0 with 2 s segments.
        let runs = Addressing::timeline_runs(&[s(Some(0), 180_000, -1)]).unwrap();
        let addressing = addressing(runs, 90_000, 0, "$Time$.m4s");
        let timing = dynamic(0, Some(10 * SEC));
        let window = addressing.window(&timing, finite(0), at(100, 0)).unwrap();
        // Ends 90..=100 (the segment ending exactly at x − B is kept).
        assert_eq!(window, 44..50);
        let last = addressing.segment(window.end - 1, 0).unwrap();
        assert_eq!(last.time, 49 * 180_000);
        assert_eq!(
            last.url.as_str(),
            "https://cdn.example.com/live/8820000.m4s"
        );
    }

    #[test]
    fn decade_old_open_timeline_is_computed_without_expansion() {
        // Epoch-anchored 1 MHz timeline 10 years in; one open run, 1 s segments.
        let ten_years = 10 * 365 * 86_400u64;
        let runs = Addressing::timeline_runs(&[s(Some(0), 1_000_000, -1)]).unwrap();
        let addressing = addressing(runs, 1_000_000, 0, "$Time$.m4s");
        let timing = dynamic(0, None);
        let now = at(ten_years as i64, 0);
        let window = addressing.window(&timing, finite(0), now).unwrap();
        assert_eq!(window, 0..ten_years);
        let segment_window = SegmentWindow::new(&addressing, 0, window).unwrap();
        let newest = segment_window.last().unwrap().unwrap();
        assert_eq!(newest.time, (ten_years - 1) * 1_000_000);
        let tail: Vec<_> = segment_window
            .iter()
            .rev()
            .take(3)
            .map(|segment| segment.unwrap().number)
            .collect();
        assert_eq!(tail, [ten_years, ten_years - 1, ten_years - 2]);
        assert_eq!(segment_window.iter().nth(5).unwrap().unwrap().number, 6);
    }

    #[test]
    fn timeline_with_presentation_time_offset_maps_to_the_period() {
        // PTO 900 at T=100 → media 900 is the Period start (P = 30 s).
        let runs = Addressing::timeline_runs(&[s(Some(700), 100, 5)]).unwrap();
        let addressing = addressing(runs, 100, 900, "$Time$.m4s");
        let timing = PeriodTiming {
            presentation_type: PresentationType::Static,
            availability_start: None,
            period_start: 30 * SEC,
            period_duration: Some(3 * SEC),
            time_shift_buffer_depth: None,
            availability_end: None,
        };
        // Media 700..1300; segments ending at or before 900 are outside the
        // Period, and the Period ends at media 1200.
        let window = addressing.window(&timing, finite(0), at(0, 0)).unwrap();
        assert_eq!(window, 2..5);
        let first = addressing.segment(2, timing.period_start).unwrap();
        assert_eq!(first.time, 900);
        assert_eq!(first.start_secs, 30.0);
        assert_eq!(first.url.as_str(), "https://cdn.example.com/live/900.m4s");
    }

    #[test]
    fn dynamic_timeline_honours_pto_in_availability() {
        // Segment media [1000, 1200) at T=100, PTO=1000, P=0 → presentation
        // [0, 2) → available at x = 2.
        let runs = Addressing::timeline_runs(&[s(Some(1000), 200, 3)]).unwrap();
        let addressing = addressing(runs, 100, 1000, "$Time$.m4s");
        let timing = dynamic(0, None);
        let window = |secs| addressing.window(&timing, finite(0), at(secs, 0)).unwrap();
        assert_eq!(window(1), 0..0);
        assert_eq!(window(2), 0..1);
        assert_eq!(window(7), 0..3);
        assert_eq!(window(100), 0..4);
    }

    #[test]
    fn end_number_caps_the_window() {
        let mut addressing = grid(2, 1, 0);
        addressing.start_number = 10;
        addressing.end_number = Some(12);
        let window = addressing
            .window(&dynamic(0, None), finite(0), at(100, 0))
            .unwrap();
        assert_eq!(window, 0..3);
        addressing.end_number = Some(5);
        assert!(matches!(
            addressing.window(&dynamic(0, None), finite(0), at(100, 0)),
            Err(ManifestError::Inconsistent { .. })
        ));
        // `@endNumber` = `u64::MAX` with `@startNumber` = 0 caps nothing.
        addressing.start_number = 0;
        addressing.end_number = Some(u64::MAX);
        let window = addressing
            .window(&dynamic(0, None), finite(0), at(100, 0))
            .unwrap();
        assert_eq!(window, 0..50);
    }

    #[test]
    fn numbers_past_u64_are_typed_errors() {
        let mut addressing = grid(2, 1, 0);
        addressing.start_number = u64::MAX - 10;
        let timing = dynamic(0, None);
        let window = addressing.segments(&timing, finite(0), at(20, 0)).unwrap();
        assert_eq!(window.numbers().end - window.numbers().start, window.len());
        assert!(matches!(
            addressing.segments(&timing, finite(0), at(30, 0)),
            Err(ManifestError::Overflow { .. })
        ));
    }

    #[test]
    fn overflowing_inputs_are_errors_not_panics() {
        let addressing = grid(1, u64::MAX, 0);
        let timing = dynamic(0, None);
        assert!(matches!(
            addressing.window(&timing, finite(0), DateTime::<Utc>::MAX_UTC),
            Err(ManifestError::Overflow { .. })
        ));
        let mut addressing = grid(2, 1, 0);
        addressing.start_number = u64::MAX;
        assert!(addressing.segment(1, 0).is_err());
        let runs = Addressing::timeline_runs(&[s(Some(u64::MAX - 1), 1, -1)]).unwrap();
        let addressing = self::addressing(runs, 1, 0, "$Time$");
        let window = addressing.window(&timing, finite(0), at(1, 0)).unwrap();
        assert!(
            window.is_empty(),
            "media time far beyond now is not yet available"
        );
        assert!(addressing.segment(5, 0).is_err());
    }

    #[test]
    fn window_accessors() {
        let mut addressing = grid(2, 1, 0);
        addressing.start_number = 100;
        let window = SegmentWindow::new(&addressing, 0, 3..6).unwrap();
        assert_eq!(window.len(), 3);
        assert_eq!(window.numbers(), 103..106);
        assert_eq!(window.first().unwrap().unwrap().number, 103);
        assert_eq!(window.last().unwrap().unwrap().number, 105);
        assert!(window.get(3).is_none());
        let numbers: Vec<_> = window.iter().map(|s| s.unwrap().number).collect();
        assert_eq!(numbers, [103, 104, 105]);
        let segment = window.get(1).unwrap().unwrap();
        assert_eq!(segment.url.as_str(), "https://cdn.example.com/live/104.m4s");
        assert_eq!(segment.duration_secs(), 2.0);
    }

    #[test]
    fn iterator_reaches_the_back_in_constant_time() {
        // A window of ~1.8e19 positions: any per-step walk would never finish.
        let addressing = grid(1, 1_000_000_000, 0);
        let window = SegmentWindow::new(&addressing, 0, 0..u64::MAX - 1).unwrap();
        let mut iter = window.iter();
        assert_eq!(iter.nth_back(9).unwrap().unwrap().number, u64::MAX - 10);
        assert_eq!(iter.next_back().unwrap().unwrap().number, u64::MAX - 11);
        assert_eq!(window.iter().last().unwrap().unwrap().number, u64::MAX - 1);
        assert_eq!(
            window.iter().count(),
            usize::try_from(u64::MAX - 1).unwrap()
        );
    }

    /// Asserts that every returned segment satisfies the availability
    /// predicate and that segment ends increase.
    fn assert_available(window: &SegmentWindow<'_>, x_secs: f64) {
        let mut previous_end = f64::NEG_INFINITY;
        for segment in window.iter() {
            let segment = segment.unwrap();
            let end = segment.start_secs + segment.duration_secs();
            assert!(
                end <= x_secs,
                "segment {} ends at {end} > {x_secs}",
                segment.number
            );
            assert!(end > previous_end, "segment ends must increase");
            previous_end = end;
        }
    }

    #[test]
    fn overlapping_open_repeat_is_clamped_to_the_next_t() {
        // Round-up gives 4 repetitions of 10 before t=35; the 4th is cut to
        // [30, 35), so ends are 10, 20, 30, 35, 36.
        let runs = Addressing::timeline_runs(&[s(Some(0), 10, -1), s(Some(35), 1, 0)]).unwrap();
        let addressing = addressing(runs, 1, 0, "$Time$.m4s");
        let timing = dynamic(0, None);
        let window = addressing.segments(&timing, finite(0), at(37, 0)).unwrap();
        let ends: Vec<_> = window
            .iter()
            .map(|segment| {
                let segment = segment.unwrap();
                segment.time + segment.duration
            })
            .collect();
        assert_eq!(ends, [10, 20, 30, 35, 36]);
        assert_available(&window, 37.0);
        for x in 0..=40 {
            let window = addressing.segments(&timing, finite(0), at(x, 0)).unwrap();
            assert_available(&window, x as f64);
        }
    }

    #[test]
    fn infinite_offset_with_finite_addressing_is_bounded() {
        let timing = dynamic(0, None);
        let inf = AvailabilityTimeOffset::Infinite;
        // A timeline without a trailing open repeat.
        let runs = Addressing::timeline_runs(&[s(Some(0), 2, 4)]).unwrap();
        let finite_timeline = addressing(runs, 1, 0, "$Time$");
        assert_eq!(
            finite_timeline.window(&timing, inf, at(0, 0)).unwrap(),
            0..5
        );
        // `@endNumber`.
        let mut capped = grid(2, 1, 0);
        capped.end_number = Some(3);
        assert_eq!(capped.window(&timing, inf, at(0, 0)).unwrap(), 0..3);
        // A SegmentList.
        let mut list = grid(2, 1, 0);
        list.urls = SegmentUrls::List {
            urls: vec![
                RawSegmentUrl {
                    media: Some("a.m4s".into()),
                    media_range: None,
                };
                4
            ]
            .into(),
            base: Url::parse("https://cdn.example.com/").unwrap(),
        };
        assert_eq!(list.window(&timing, inf, at(0, 0)).unwrap(), 0..4);
        assert_eq!(
            list.segment(3, 0).unwrap().url.as_str(),
            "https://cdn.example.com/a.m4s"
        );
        // A static presentation without a Period duration but with a
        // finite timeline.
        let static_open = PeriodTiming {
            presentation_type: PresentationType::Static,
            availability_start: None,
            ..timing
        };
        assert_eq!(
            finite_timeline
                .window(&static_open, finite(0), at(0, 0))
                .unwrap(),
            0..5
        );
    }

    #[test]
    fn nothing_is_available_after_availability_end_time() {
        let addressing = grid(2, 1, 0);
        let timing = PeriodTiming {
            availability_end: Some(20 * SEC),
            ..dynamic(0, None)
        };
        assert_eq!(
            addressing.window(&timing, finite(0), at(20, 0)).unwrap(),
            0..10
        );
        assert!(
            addressing
                .window(&timing, finite(0), at(20, 1))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn timeline_ahead_reaches_the_newest_listed_segment_exactly() {
        // 12288 ticks/s, so segment ends are not whole nanoseconds. The
        // newest entry ends at 12 312 577 / 12288 s ≈ 1002.0000814 s.
        const T: u64 = 12288;
        let runs =
            Addressing::timeline_runs(&[s(Some(1000 * T), T, 0), s(None, T + 1, 0)]).unwrap();
        let mut addressing = addressing(runs, T, 0, "$Number$.m4s");
        addressing.has_timeline = true;
        let timing = dynamic(0, Some(5 * SEC));
        let now = at(10, 0);
        let ahead = addressing
            .timeline_ahead(&timing, finite(1), now)
            .unwrap()
            .expect("everything listed is beyond x + A = 11 s");
        assert_eq!(ahead.earliest, Duration::from_secs(990));
        assert_eq!(ahead.newest, Duration::from_nanos(991_000_081_381));
        assert_eq!(ahead.segment_duration, Duration::from_secs(1));

        let reached = now + chrono::TimeDelta::from_std(ahead.newest).unwrap();
        assert_eq!(
            addressing.window(&timing, finite(1), reached).unwrap(),
            0..2
        );
        let before = reached - chrono::TimeDelta::nanoseconds(1);
        assert_eq!(addressing.window(&timing, finite(1), before).unwrap(), 0..1);
        assert_eq!(
            addressing
                .timeline_ahead(&timing, finite(1), reached)
                .unwrap(),
            None,
            "available now"
        );

        assert_eq!(
            addressing
                .timeline_ahead(&timing, AvailabilityTimeOffset::Infinite, now)
                .unwrap(),
            None
        );
        let static_timing = PeriodTiming {
            presentation_type: PresentationType::Static,
            ..timing
        };
        assert_eq!(
            addressing
                .timeline_ahead(&static_timing, finite(1), now)
                .unwrap(),
            None
        );
        addressing.has_timeline = false;
        assert_eq!(
            addressing.timeline_ahead(&timing, finite(1), now).unwrap(),
            None
        );
    }

    #[test]
    fn timeline_ahead_of_an_open_run_counts_only_its_listed_segment() {
        let runs = Addressing::timeline_runs(&[s(Some(100), 2, -1)]).unwrap();
        let mut addressing = addressing(runs, 1, 0, "$Number$.m4s");
        addressing.has_timeline = true;
        // The first repetition ends at 102 s; the edge is at 10 + 0 s.
        let ahead = addressing
            .timeline_ahead(&dynamic(0, None), finite(0), at(10, 0))
            .unwrap()
            .unwrap();
        assert_eq!(ahead.earliest, Duration::from_secs(92));
        assert_eq!(ahead.newest, Duration::from_secs(92));
        assert_eq!(ahead.segment_duration, Duration::from_secs(2));
    }

    #[test]
    fn timeline_ahead_counts_only_segments_the_addressing_allows() {
        let timeline = |start: u64| {
            let runs = Addressing::timeline_runs(&[s(Some(start), 2, 100)]).unwrap();
            let mut addressing = addressing(runs, 1, 0, "$Number$.m4s");
            addressing.has_timeline = true;
            addressing.end_number = Some(4);
            addressing
        };
        let timing = dynamic(0, Some(12 * SEC));
        // Segments 1–4 end at 2–8 s; the timeline's 101st would end at 202 s.
        let allowed = timeline(0);
        assert_eq!(
            allowed
                .timeline_ahead(&timing, finite(0), at(6, 0))
                .unwrap(),
            None
        );
        assert_eq!(allowed.window(&timing, finite(0), at(6, 0)).unwrap(), 0..3);

        // Far ahead, the newest listed segment is the fourth (1008 s).
        let ahead = timeline(1000)
            .timeline_ahead(&timing, finite(0), at(6, 0))
            .unwrap()
            .unwrap();
        assert_eq!(ahead.earliest, Duration::from_secs(996));
        assert_eq!(ahead.newest, Duration::from_secs(1002));

        // `availabilityEndTime` bounds how far ahead availability is taken:
        // at 20 s with the end at 1005 s, the segments ending at 1002 and
        // 1004 s are still available before it.
        let ending = PeriodTiming {
            availability_end: Some(1005 * SEC),
            ..timing
        };
        let addressing = timeline(1000);
        let ahead = addressing
            .timeline_ahead(&ending, finite(0), at(20, 0))
            .unwrap()
            .unwrap();
        assert_eq!(ahead.newest, Duration::from_secs(985));
        let reached = at(20, 0) + chrono::TimeDelta::from_std(ahead.newest).unwrap();
        assert_eq!(
            addressing.window(&ending, finite(0), reached).unwrap(),
            0..2
        );
        // Nothing listed becomes available before an earlier end.
        let early = PeriodTiming {
            availability_end: Some(1001 * SEC),
            ..timing
        };
        assert_eq!(
            addressing
                .timeline_ahead(&early, finite(0), at(20, 0))
                .unwrap(),
            None
        );

        // Nothing is listed past `availabilityEndTime`.
        let ended = PeriodTiming {
            availability_end: Some(5 * SEC),
            ..timing
        };
        assert_eq!(
            timeline(1000)
                .timeline_ahead(&ended, finite(0), at(6, 0))
                .unwrap(),
            None
        );
    }

    #[test]
    fn timeline_ahead_ignores_periods_not_running_by_the_clock() {
        let runs = Addressing::timeline_runs(&[s(Some(0), 2, 4)]).unwrap();
        let mut addressing = addressing(runs, 1, 0, "$Number$.m4s");
        addressing.has_timeline = true;
        // A Period starting at 100 s, preannounced at 20 s.
        let future = dynamic(100 * SEC, None);
        assert_eq!(
            addressing
                .timeline_ahead(&future, finite(0), at(20, 0))
                .unwrap(),
            None
        );
        // A Period that ended at 50 s.
        let past = PeriodTiming {
            period_duration: Some(50 * SEC),
            ..dynamic(0, None)
        };
        let runs = Addressing::timeline_runs(&[s(Some(1000), 2, 4)]).unwrap();
        addressing.runs = runs.into();
        assert_eq!(
            addressing
                .timeline_ahead(&past, finite(0), at(60, 0))
                .unwrap(),
            None
        );
        // While it runs, entries past its end are not listed either.
        assert_eq!(
            addressing
                .timeline_ahead(&past, finite(0), at(20, 0))
                .unwrap(),
            None
        );
    }
}
