//! Indexed addressing (`SegmentBase`, ISO/IEC 23009-1 §5.3.9.2): the
//! Representation is one resource whose subsegments are listed by a Segment
//! Index (`sidx`, ISO/IEC 14496-12 §8.16.3). Each subsegment becomes one
//! segment: a byte range of the resource with its own duration, so the
//! segments form a timeline like an `S` per subsegment.
//!
//! Reading the index is I/O, done by the engine before planning; this
//! module turns what it read into segment addressing.

use std::sync::Arc;

use url::Url;

use super::error::{ManifestError, UnsupportedFeature};
use super::limits::MAX_SUBSEGMENTS;
use super::timeline::{Addressing, ByteRange, Run, SegmentUrls};

/// Where a `SegmentBase` Representation's Segment Index is: `@indexRange`
/// of the resource at `url`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SegmentIndexLocation {
    pub url: Url,
    pub range: ByteRange,
}

/// One subsegment: where its bytes are and how long it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subsegment {
    pub range: ByteRange,
    pub duration: u64,
}

/// The subsegments of an indexed resource, validated and ready to address.
/// Cheap to clone: every Representation (in any Period) indexed by the same
/// resource shares one copy.
#[derive(Debug, Clone)]
pub struct SubsegmentIndex {
    timescale: u64,
    /// File offset of the top-level `sidx` box.
    index_offset: u64,
    /// Equal consecutive durations share a run, as `S@r` would.
    runs: Arc<[Run]>,
    ranges: Arc<[ByteRange]>,
}

impl SubsegmentIndex {
    /// Subsegments in presentation order, the first starting at
    /// `earliest_presentation_time`; times are in `timescale` ticks per
    /// second. Each must have a duration and a byte range after the
    /// previous one's. `index_offset` is where the top-level `sidx` box
    /// starts in the resource: a self-initializing resource's initialization
    /// data ends there.
    pub fn new(
        timescale: u64,
        earliest_presentation_time: u64,
        index_offset: u64,
        subsegments: &[Subsegment],
    ) -> Result<Self, ManifestError> {
        if subsegments.is_empty() {
            return Err(ManifestError::segment_index("it lists no subsegments"));
        }
        if subsegments.len() > MAX_SUBSEGMENTS {
            return Err(ManifestError::LimitExceeded {
                what: "subsegments per Segment Index",
                limit: MAX_SUBSEGMENTS,
            });
        }
        if timescale == 0 {
            return Err(ManifestError::segment_index("timescale is zero"));
        }
        let overflow = || ManifestError::overflow("Segment Index timeline");
        let mut runs: Vec<Run> = Vec::new();
        let mut ranges = Vec::with_capacity(subsegments.len());
        let mut time = earliest_presentation_time;
        let mut previous_last: Option<u64> = None;
        for (position, subsegment) in subsegments.iter().enumerate() {
            if subsegment.duration == 0 {
                return Err(ManifestError::segment_index(format!(
                    "subsegment {position} has no duration"
                )));
            }
            let range = subsegment.range;
            if range.first > range.last || previous_last.is_some_and(|last| range.first <= last) {
                return Err(ManifestError::segment_index(format!(
                    "subsegment {position} has an empty byte range or overlaps the one before"
                )));
            }
            previous_last = Some(range.last);
            match runs.last_mut() {
                Some(run) if run.duration == subsegment.duration => {
                    run.count = run.count.map(|count| count + 1);
                }
                _ => runs.push(Run {
                    start: time,
                    duration: subsegment.duration,
                    count: Some(1),
                    first_index: u64::try_from(position).map_err(|_| overflow())?,
                }),
            }
            time = time.checked_add(subsegment.duration).ok_or_else(overflow)?;
            ranges.push(range);
        }
        Ok(Self {
            timescale,
            index_offset,
            runs: runs.into(),
            ranges: ranges.into(),
        })
    }

    pub fn timescale(&self) -> u64 {
        self.timescale
    }

    pub fn index_offset(&self) -> u64 {
        self.index_offset
    }

    /// Number of subsegments.
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Never true for a constructed index; for symmetry with [`Self::len`].
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

/// The addressing of `pending` (a `SegmentBase` Representation's, carrying
/// the MPD's timescale and presentation time offset) once its index is
/// known.
///
/// Segment times are in the index's timescale, so the presentation time
/// offset is converted to it; an offset that is not a whole number of index
/// ticks cannot be placed on the subsegment timeline exactly and is not
/// supported.
pub(crate) fn indexed_addressing(
    index: &SubsegmentIndex,
    pending: &Addressing,
    url: Url,
) -> Result<Addressing, ManifestError> {
    let presentation_time_offset = if pending.timescale == index.timescale {
        pending.presentation_time_offset
    } else {
        let scaled = u128::from(pending.presentation_time_offset) * u128::from(index.timescale);
        let divisor = u128::from(pending.timescale.max(1));
        if scaled % divisor != 0 {
            return Err(UnsupportedFeature::FractionalPresentationTimeOffset.into());
        }
        u64::try_from(scaled / divisor)
            .map_err(|_| ManifestError::overflow("presentationTimeOffset"))?
    };
    Ok(Addressing {
        timescale: index.timescale,
        presentation_time_offset,
        start_number: 1,
        end_number: None,
        has_timeline: true,
        runs: Arc::clone(&index.runs),
        urls: SegmentUrls::Indexed {
            url,
            ranges: Arc::clone(&index.ranges),
        },
    })
}
