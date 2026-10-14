//! MPEG-DASH Media Presentation Description (ISO/IEC 23009-1): parsing,
//! resolution and segment availability.
//!
//! [`Manifest::parse`] turns an MPD body into a resolved presentation: Period
//! timing is derived, attributes and segment information are inherited from
//! Period to AdaptationSet to Representation, and BaseURLs are resolved. Each
//! [`Representation`] then answers which segments are available at a given
//! server time ([`Representation::segments`]); the caller obtains that time
//! from a [`Clock`] corrected by a [`ClockOffset`].
//!
//! Scope: `SegmentTemplate` (`$Number$`, `$Time$`, `SegmentTimeline`) and
//! `SegmentList` addressing of unencrypted ISO BMFF content, for static and
//! dynamic MPDs, and `SegmentBase` addressing for static MPDs. A
//! `SegmentBase` Representation's segments come from its Segment Index,
//! which the caller reads ([`Representation::pending_segment_index`]) and
//! applies ([`Representation::apply_segment_index`]). Content outside that scope is reported per AdaptationSet or
//! Representation in [`Period::rejected`], or as
//! [`ManifestError::Unsupported`] when it affects the whole MPD; it is never
//! approximated.
//!
//! Nothing here performs I/O. Clause numbers in this module refer to
//! ISO/IEC 23009-1:2019 (4th edition).

mod clock;
mod error;
mod index;
pub mod limits;
mod parse;
mod resolve;
mod template;
mod timeline;
mod xs;

pub use clock::{Clock, ClockOffset, ManualClock, SystemClock, UtcTiming, parse_http_date};
pub use error::{ManifestError, UnsupportedFeature};
pub use index::{SegmentIndexLocation, Subsegment, SubsegmentIndex};
pub use limits::MAX_MPD_BYTES;
pub use resolve::{
    AdaptationSet, ContentType, Manifest, Period, PresentationType, Rejected, RejectedContent,
    Representation,
};
pub(crate) use timeline::NANOS_PER_SEC;
pub use timeline::{
    AddressingKind, AvailabilityTimeOffset, ByteRange, InitSegment, Segment, SegmentIter,
    SegmentWindow, TimelineLead,
};
