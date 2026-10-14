//! Raw MPD → resolved presentation: Period timing, attribute and segment
//! information inheritance, BaseURL resolution and v1 scope checks.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use url::Url;

use crate::error::SharedError;

use super::clock::UtcTiming;
use super::error::{ManifestError, UnsupportedFeature};
use super::index::{SegmentIndexLocation, SubsegmentIndex, indexed_addressing};
use super::limits::{MAX_RESOLVED_BYTES, MAX_URL_BYTES};
use super::parse::{
    self, RawAdaptationSet, RawBaseUrl, RawCommon, RawMpd, RawMultipleSegmentBase, RawPeriod,
    RawProperty, RawRepresentation, RawS, RawSegmentBase, RawSegmentInfo, RawSegmentList,
    RawSegmentTemplate, RawUrl,
};
use super::template::{Template, TemplateKind};
use super::timeline::{
    Addressing, AddressingKind, AvailabilityTimeOffset, ByteRange, InitSegment, PeriodTiming, Run,
    SegmentUrls, SegmentWindow, TimelineLead,
};
use super::xs::{self, RawOffset};

/// `EssentialProperty` schemes that do not affect fetching or storing
/// segments: colour signalling (ISO/IEC 23001-8 CICP) carried in the
/// bitstream anyway. Any other `EssentialProperty` makes its element unusable
/// (ISO/IEC 23009-1 §5.8.4, EssentialProperty).
const UNDERSTOOD_ESSENTIAL_PROPERTIES: &[&str] = &[
    "urn:mpeg:mpegB:cicp:ColourPrimaries",
    "urn:mpeg:mpegB:cicp:TransferCharacteristics",
    "urn:mpeg:mpegB:cicp:MatrixCoefficients",
];

/// MPD chaining and fallback chaining (ISO/IEC 23009-1, 4th edition).
const CHAINING_SCHEMES: &[&str] = &[
    "urn:mpeg:dash:mpd-chaining:2016",
    "urn:mpeg:dash:fallback:2016",
];

/// Bytes charged per Representation on top of its strings, for the fixed
/// parts of the resolved model.
const REPRESENTATION_OVERHEAD: usize = 256;

/// `MPD@type` (ISO/IEC 23009-1 §5.3.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationType {
    /// On demand: every segment of every Period is available.
    Static,
    /// Live: segments become available over time and the MPD may change.
    Dynamic,
}

/// The media type of a Representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Video,
    Audio,
}

/// A resolved Media Presentation Description.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub presentation_type: PresentationType,
    /// Anchor of the MPD timeline for dynamic presentations; always present
    /// when `presentation_type` is [`PresentationType::Dynamic`].
    pub availability_start_time: Option<DateTime<Utc>>,
    /// After this time no segment is available
    /// ([`Representation::segments`] is empty): the presentation has ended.
    pub availability_end_time: Option<DateTime<Utc>>,
    pub publish_time: Option<DateTime<Utc>>,
    pub media_presentation_duration: Option<Duration>,
    pub minimum_update_period: Option<Duration>,
    pub min_buffer_time: Option<Duration>,
    pub time_shift_buffer_depth: Option<Duration>,
    pub suggested_presentation_delay: Option<Duration>,
    pub max_segment_duration: Option<Duration>,
    /// `Location`: where to fetch the next MPD update, if it moved.
    pub location: Option<Url>,
    /// Supported clock sources, in document order.
    pub utc_timings: Vec<UtcTiming>,
    /// Periods with a known start, in presentation order.
    pub periods: Vec<Period>,
    /// Trailing Periods of a dynamic MPD whose start is not yet known
    /// ("early available", ISO/IEC 23009-1 §5.3.2.1); not part of `periods`.
    pub early_available_periods: usize,
    /// Alternative `BaseURL`s (sibling ones after the first, at any level)
    /// that are not used: only the first `BaseURL` at each level is.
    pub ignored_base_urls: usize,
}

/// A resolved Period.
///
/// Identity across MPD updates: `id` when present (unique within the MPD),
/// otherwise the exact `start`.
#[derive(Debug, Clone)]
pub struct Period {
    pub id: Option<String>,
    /// Start on the MPD timeline.
    pub start: Duration,
    /// Extent of the Period; `None` while a dynamic Period is open.
    pub duration: Option<Duration>,
    /// Adaptation sets with at least one supported Representation.
    pub adaptation_sets: Vec<AdaptationSet>,
    /// What was left out of `adaptation_sets` and why.
    pub rejected: Vec<Rejected>,
}

impl Period {
    /// Fails when the Period has no usable Representation, reporting the
    /// first unsupported feature that excluded one.
    pub fn ensure_usable(&self) -> Result<(), ManifestError> {
        if self.adaptation_sets.is_empty() {
            return Err(match self.rejected.first() {
                Some(rejected) => rejected.feature.clone().into(),
                None => ManifestError::inconsistent("Period has no Representation"),
            });
        }
        Ok(())
    }
}

/// An AdaptationSet or Representation excluded as out of scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub adaptation_set_id: Option<String>,
    /// `None` when the whole AdaptationSet was rejected.
    pub representation_id: Option<String>,
    /// What the element would have carried, so a rejection can be
    /// attributed to the track it would have served.
    pub content: RejectedContent,
    pub feature: UnsupportedFeature,
}

/// The media type of a rejected AdaptationSet or Representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectedContent {
    Video,
    Audio,
    /// Declared as something else: text (subtitles), images, metadata.
    Other,
    /// Nothing declares the type.
    Unknown,
}

/// What `@contentType`, else the MIME type (with `@codecs` for ISO BMFF
/// carried text), declares.
fn declared_content(set: &RawAdaptationSet, common: Option<&RawCommon>) -> RejectedContent {
    match set.content_type.as_deref() {
        Some("video") => return RejectedContent::Video,
        Some("audio") => return RejectedContent::Audio,
        Some(_) => return RejectedContent::Other,
        None => {}
    }
    let Some(mime) = common
        .and_then(|common| common.mime_type.as_deref())
        .or(set.common.mime_type.as_deref())
    else {
        return RejectedContent::Unknown;
    };
    let mime = mime.trim_start().to_ascii_lowercase();
    let media = mime.split('/').next().unwrap_or("");
    match media {
        "video" => RejectedContent::Video,
        "audio" => RejectedContent::Audio,
        "text" | "image" => RejectedContent::Other,
        "application" => {
            let codecs = common
                .and_then(|common| common.codecs.as_deref())
                .or(set.common.codecs.as_deref())
                .unwrap_or("");
            let text_codec = codecs
                .split(',')
                .map(|codec| codec.trim().to_ascii_lowercase())
                .any(|codec| {
                    ["stpp", "wvtt", "tx3g", "c608", "evte"]
                        .iter()
                        .any(|text| codec.starts_with(text))
                });
            if text_codec || mime.contains("ttml") || mime.contains("vtt") {
                RejectedContent::Other
            } else {
                RejectedContent::Unknown
            }
        }
        _ => RejectedContent::Unknown,
    }
}

/// A resolved AdaptationSet.
#[derive(Debug, Clone)]
pub struct AdaptationSet {
    /// Unique within the Period when present.
    pub id: Option<String>,
    pub lang: Option<String>,
    pub segment_alignment: bool,
    pub representations: Vec<Representation>,
}

/// A resolved, supported Representation.
#[derive(Debug, Clone)]
pub struct Representation {
    /// Unique within the Period.
    pub id: String,
    pub bandwidth: u64,
    pub content_type: ContentType,
    pub mime_type: String,
    pub codecs: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub start_with_sap: Option<u8>,
    /// Sum of the offsets of the BaseURLs used to resolve this Representation
    /// and of its segment information (DASH-IF timing model §13.3,
    /// ISO/IEC 23009-1 §5.3.9.5.3).
    pub availability_time_offset: AvailabilityTimeOffset,
    /// `false` when any applicable `@availabilityTimeComplete` is `false`
    /// (chunked low-latency delivery).
    pub availability_time_complete: bool,
    pub initialization: Option<InitSegment>,
    addressing: Addressing,
    /// Set while a `SegmentBase` Representation's Segment Index is unread.
    segment_index: Option<PendingIndex>,
    timing: PeriodTiming,
}

impl Representation {
    pub fn timescale(&self) -> u64 {
        self.addressing.timescale
    }

    pub fn presentation_time_offset(&self) -> u64 {
        self.addressing.presentation_time_offset
    }

    /// How segment URLs are formed.
    pub fn addressing_kind(&self) -> AddressingKind {
        self.addressing.kind()
    }

    /// Whether segment timing comes from a `SegmentTimeline`. Together with
    /// [`Self::addressing_kind`] it decides a segment's stable identity
    /// across MPD updates (see [`super::AddressingKind`]).
    pub fn uses_segment_timeline(&self) -> bool {
        self.addressing.has_timeline
    }

    /// The segments available at `now` (the server's time; see
    /// [`super::ClockOffset::apply`]). For static presentations, every segment
    /// of the Period.
    ///
    /// When `availability_time_complete` is false, a segment is only complete
    /// at its unadjusted availability time, so the availability time offset
    /// is not applied: this engine downloads whole segments.
    ///
    /// Fails while the Segment Index is unread (see
    /// [`Self::pending_segment_index`]).
    pub fn segments(&self, now: DateTime<Utc>) -> Result<SegmentWindow<'_>, ManifestError> {
        if let Some(error) = self.unread_segment_index_error() {
            return Err(error);
        }
        self.addressing
            .segments(&self.timing, self.effective_offset(), now)
    }

    /// The availability time offset segments become available by: none
    /// while a segment is only complete at its unadjusted availability time
    /// (see [`Self::segments`]).
    fn effective_offset(&self) -> AvailabilityTimeOffset {
        if self.availability_time_complete {
            self.availability_time_offset
        } else {
            AvailabilityTimeOffset::ZERO
        }
    }

    /// How far ahead of the segments available at `now` everything this
    /// Representation's `SegmentTimeline` lists lies, when not one listed
    /// segment is available yet and the Period is running by the clock
    /// (see [`Self::segments`] for the availability rules).
    ///
    /// An MPD may describe segments before they become available, so this
    /// alone does not prove the clock wrong; what it means is the caller's
    /// decision.
    pub fn timeline_ahead(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<TimelineLead>, ManifestError> {
        self.addressing
            .timeline_ahead(&self.timing, self.effective_offset(), now)
    }

    /// The first segment this Representation's `SegmentTimeline` lists
    /// from segment `number` on, as `(number, media time)`, regardless of
    /// availability; `None` without a timeline or past its end.
    pub fn listed_from(&self, number: u64) -> Option<(u64, u64)> {
        self.addressing.listed_from(number)
    }

    /// Where this `SegmentBase` Representation's Segment Index is, while it
    /// has not been applied. Its segments are unknown until then.
    pub fn pending_segment_index(&self) -> Option<&SegmentIndexLocation> {
        self.segment_index.as_ref().map(|pending| &pending.location)
    }

    /// Why this Representation's segments are unknown while its Segment
    /// Index is unread: the read's failure, once one was recorded with
    /// [`Self::fail_segment_index`].
    pub fn unread_segment_index_error(&self) -> Option<ManifestError> {
        let pending = self.segment_index.as_ref()?;
        let representation = self.id.clone();
        Some(match &pending.unreadable {
            Some(cause) => ManifestError::SegmentIndexUnreadable {
                representation,
                cause: cause.clone(),
            },
            None => ManifestError::SegmentIndexPending { representation },
        })
    }

    /// Records why the pending Segment Index could not be read or applied;
    /// the Representation stays without segments.
    pub fn fail_segment_index(&mut self, cause: SharedError) {
        if let Some(pending) = &mut self.segment_index {
            pending.unreadable = Some(cause);
        }
    }

    /// Gives a `SegmentBase` Representation the subsegments read from its
    /// Segment Index; each becomes one segment. Segment times are then in the
    /// index's timescale ([`Self::timescale`]).
    ///
    /// Without an `Initialization` element the resource is self-initializing
    /// and its initialization data is everything before the index box, which
    /// is only known now: `@indexRange` may begin before the box.
    pub fn apply_segment_index(&mut self, index: &SubsegmentIndex) -> Result<(), ManifestError> {
        let Some(pending) = &self.segment_index else {
            return Err(ManifestError::inconsistent(
                "Representation is not waiting for a Segment Index",
            ));
        };
        let url = pending.location.url.clone();
        self.addressing = indexed_addressing(index, &self.addressing, url.clone())?;
        if pending.implicit_initialization {
            self.initialization = index.index_offset().checked_sub(1).map(|last| InitSegment {
                url,
                byte_range: Some(ByteRange { first: 0, last }),
            });
        }
        self.segment_index = None;
        Ok(())
    }
}

impl Manifest {
    /// Parses and resolves an MPD fetched from `document_url` (the final URL
    /// after redirects, which relative BaseURLs resolve against).
    pub fn parse(body: &[u8], document_url: &Url) -> Result<Self, ManifestError> {
        Resolver::default().resolve(parse::parse_mpd(body)?, document_url)
    }
}

/// State shared across the resolution of one MPD.
#[derive(Default)]
struct Resolver {
    /// Bytes of per-Representation data created so far (see
    /// [`MAX_RESOLVED_BYTES`]).
    resolved_bytes: usize,
    /// Runs per distinct `SegmentTimeline`, keyed by its shared allocation,
    /// so inheriting Representations share one copy.
    runs: HashMap<*const RawS, Arc<[Run]>>,
}

impl Resolver {
    fn charge(&mut self, bytes: usize) -> Result<(), ManifestError> {
        self.resolved_bytes = self
            .resolved_bytes
            .checked_add(bytes)
            .filter(|&total| total <= MAX_RESOLVED_BYTES)
            .ok_or(ManifestError::LimitExceeded {
                what: "resolved MPD data",
                limit: MAX_RESOLVED_BYTES,
            })?;
        Ok(())
    }

    fn timeline_runs(&mut self, timeline: &Arc<[RawS]>) -> Result<Arc<[Run]>, ManifestError> {
        let key = Arc::as_ptr(timeline).cast::<RawS>();
        if let Some(runs) = self.runs.get(&key) {
            return Ok(Arc::clone(runs));
        }
        let runs: Arc<[Run]> = Addressing::timeline_runs(timeline)?.into();
        self.runs.insert(key, Arc::clone(&runs));
        Ok(runs)
    }

    fn resolve(mut self, raw: RawMpd, document_url: &Url) -> Result<Manifest, ManifestError> {
        let dynamic = raw.presentation_type == PresentationType::Dynamic;
        if dynamic && raw.availability_start_time.is_none() {
            return Err(ManifestError::MissingAttribute {
                element: "MPD",
                attribute: "availabilityStartTime",
            });
        }
        check_properties(&raw.properties)?;
        let mut period_ids = HashSet::new();
        if raw
            .periods
            .iter()
            .filter_map(|period| period.id.as_deref())
            .any(|id| !period_ids.insert(id))
        {
            return Err(ManifestError::inconsistent("duplicate Period@id"));
        }
        let mpd_base = BaseChain::new(document_url.clone()).join(raw.base_url.as_ref())?;

        // Period starts (ISO/IEC 23009-1 §5.3.2.1): explicit `@start`; else the
        // previous Period's start plus its `@duration`; else 0 for the first
        // Period of a static MPD. A dynamic Period whose start cannot be
        // derived is early available and not yet on the timeline.
        let mut timed: Vec<(RawPeriod, Duration)> = Vec::with_capacity(raw.periods.len());
        let mut previous: Option<(Duration, Option<Duration>)> = None;
        let mut early_available_periods = 0;
        for (position, period) in raw.periods.into_iter().enumerate() {
            if period.has_xlink {
                return Err(UnsupportedFeature::XLink { element: "Period" }.into());
            }
            let start = match (period.start, previous) {
                (Some(start), _) => Some(start),
                (None, Some((start, Some(duration)))) => Some(
                    start
                        .checked_add(duration)
                        .ok_or(ManifestError::overflow("Period start"))?,
                ),
                (None, None) if position == 0 && !dynamic => Some(Duration::ZERO),
                _ => None,
            };
            match start {
                Some(start) => {
                    if let Some((_, last_start)) = timed.last()
                        && start < *last_start
                    {
                        return Err(ManifestError::inconsistent(format!(
                            "Period {position} starts before the previous Period"
                        )));
                    }
                    previous = Some((start, period.duration));
                    timed.push((period, start));
                }
                None if dynamic => {
                    early_available_periods += 1;
                    previous = None;
                }
                None => {
                    return Err(ManifestError::inconsistent(format!(
                        "the start of Period {position} cannot be derived"
                    )));
                }
            }
        }

        // Period durations: up to the next Period's start; for the last Period,
        // up to `mediaPresentationDuration`, else its own `@duration`, else open.
        let starts: Vec<Duration> = timed.iter().map(|(_, start)| *start).collect();
        let mut periods = Vec::with_capacity(timed.len());
        for (index, (period, start)) in timed.into_iter().enumerate() {
            let duration = match starts.get(index + 1) {
                Some(next) => Some(next.saturating_sub(start)),
                None => match raw.media_presentation_duration {
                    Some(total) => Some(total.checked_sub(start).ok_or_else(|| {
                        ManifestError::inconsistent(
                            "mediaPresentationDuration ends before the last Period starts",
                        )
                    })?),
                    None => period.duration,
                },
            };
            let timing = PeriodTiming {
                presentation_type: raw.presentation_type,
                availability_start: raw.availability_start_time.map(xs::instant_nanos),
                period_start: xs::duration_nanos(start),
                period_duration: duration.map(xs::duration_nanos),
                time_shift_buffer_depth: raw.time_shift_buffer_depth.map(xs::duration_nanos),
                availability_end: raw.availability_end_time.map(xs::instant_nanos),
            };
            periods.push(self.resolve_period(period, start, duration, timing, &mpd_base)?);
        }

        let utc_timings = raw
            .utc_timings
            .iter()
            .filter_map(|timing| {
                UtcTiming::from_descriptor(
                    &timing.scheme_id_uri,
                    timing.value.as_deref(),
                    document_url,
                )
            })
            .collect();
        // An unusable `Location` only means updates keep using the current URL.
        let location = raw
            .location
            .filter(|location| !location.is_empty())
            .and_then(|location| document_url.join(&location).ok());

        Ok(Manifest {
            presentation_type: raw.presentation_type,
            availability_start_time: raw.availability_start_time,
            availability_end_time: raw.availability_end_time,
            publish_time: raw.publish_time,
            media_presentation_duration: raw.media_presentation_duration,
            minimum_update_period: raw.minimum_update_period,
            min_buffer_time: raw.min_buffer_time,
            time_shift_buffer_depth: raw.time_shift_buffer_depth,
            suggested_presentation_delay: raw.suggested_presentation_delay,
            max_segment_duration: raw.max_segment_duration,
            location,
            utc_timings,
            periods,
            early_available_periods,
            ignored_base_urls: raw.ignored_base_urls,
        })
    }

    fn resolve_period(
        &mut self,
        period: RawPeriod,
        start: Duration,
        duration: Option<Duration>,
        timing: PeriodTiming,
        mpd_base: &BaseChain,
    ) -> Result<Period, ManifestError> {
        check_properties(&period.properties)?;
        // AdaptationSet@id and Representation@id are unique within a Period
        // (ISO/IEC 23009-1 §5.3.3.2, §5.3.5.2); they identify the selection
        // across MPD updates.
        let mut set_ids = HashSet::new();
        if period
            .adaptation_sets
            .iter()
            .filter_map(|set| set.id.as_deref())
            .any(|id| !set_ids.insert(id))
        {
            return Err(ManifestError::inconsistent(
                "duplicate AdaptationSet@id in a Period",
            ));
        }
        let mut ids = HashSet::new();
        if period
            .adaptation_sets
            .iter()
            .flat_map(|set| &set.representations)
            .any(|representation| !ids.insert(representation.id.as_str()))
        {
            return Err(ManifestError::inconsistent(
                "duplicate Representation@id in a Period",
            ));
        }

        let base = mpd_base.join(period.base_url.as_ref())?;
        let mut adaptation_sets = Vec::new();
        let mut rejected = Vec::new();
        for mut set in period.adaptation_sets {
            let representations = std::mem::take(&mut set.representations);
            self.charge(set.id.as_ref().map_or(0, String::len))?;
            let set_base = match check_set(&set, &base) {
                Ok(set_base) => set_base,
                Err(ManifestError::Unsupported(feature)) => {
                    let content = declared_content(
                        &set,
                        representations
                            .first()
                            .map(|representation| &representation.common),
                    );
                    rejected.push(Rejected {
                        adaptation_set_id: set.id.clone(),
                        representation_id: None,
                        content,
                        feature,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut supported = Vec::new();
            for representation in representations {
                self.charge(REPRESENTATION_OVERHEAD + 2 * representation.id.len())?;
                let id = representation.id.clone();
                let content = declared_content(&set, Some(&representation.common));
                let context = SetContext {
                    set: &set,
                    period_info: &period.segment_info,
                    timing,
                };
                match self.resolve_representation(&context, representation, &set_base) {
                    Ok(representation) => supported.push(representation),
                    Err(ManifestError::Unsupported(feature)) => {
                        self.charge(set.id.as_ref().map_or(0, String::len))?;
                        rejected.push(Rejected {
                            adaptation_set_id: set.id.clone(),
                            representation_id: Some(id),
                            content,
                            feature,
                        });
                    }
                    Err(error) => return Err(error),
                }
            }
            if !supported.is_empty() {
                self.charge(set.lang.as_ref().map_or(0, String::len))?;
                adaptation_sets.push(AdaptationSet {
                    id: set.id,
                    lang: set.lang,
                    segment_alignment: set.segment_alignment,
                    representations: supported,
                });
            }
        }

        Ok(Period {
            id: period.id,
            start,
            duration,
            adaptation_sets,
            rejected,
        })
    }

    fn resolve_representation(
        &mut self,
        context: &SetContext<'_>,
        representation: RawRepresentation,
        set_base: &BaseChain,
    ) -> Result<Representation, ManifestError> {
        let set = context.set;
        check_descriptors(&representation.common)?;

        // Common attributes are inherited from the AdaptationSet
        // (ISO/IEC 23009-1 §5.3.7).
        let common = &representation.common;
        let mime_type = common
            .mime_type
            .as_ref()
            .or(set.common.mime_type.as_ref())
            .ok_or(ManifestError::MissingAttribute {
                element: "Representation",
                attribute: "mimeType",
            })?;
        let container = mime_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let content_type = match container.as_str() {
            "video/mp4" => ContentType::Video,
            "audio/mp4" => ContentType::Audio,
            _ => {
                let feature = match declared_content(set, Some(common)) {
                    RejectedContent::Other => UnsupportedFeature::NotAudioVideo(mime_type.clone()),
                    _ => UnsupportedFeature::MimeType(mime_type.clone()),
                };
                return Err(feature.into());
            }
        };
        let declared = match set.content_type.as_deref() {
            None => None,
            Some("video") => Some(ContentType::Video),
            Some("audio") => Some(ContentType::Audio),
            Some(_) => {
                return Err(ManifestError::inconsistent(
                    "AdaptationSet@contentType contradicts the ISO BMFF audio/video mimeType",
                ));
            }
        };
        if declared.is_some_and(|declared| declared != content_type) {
            return Err(ManifestError::inconsistent(
                "AdaptationSet@contentType contradicts the mimeType",
            ));
        }

        let base = set_base.join(representation.base_url.as_ref())?;
        let info = merge_segment_info([
            context.period_info,
            &set.segment_info,
            &representation.segment_info,
        ])?;
        let built = match &info {
            SegmentInfo::Base(segment_base) => {
                if context.timing.presentation_type == PresentationType::Dynamic {
                    return Err(UnsupportedFeature::LiveSegmentBase.into());
                }
                build_indexed(segment_base, &base.url)?
            }
            SegmentInfo::Multiple(info) => self.build_addressing(
                info,
                &representation.id,
                representation.bandwidth,
                &base.url,
            )?,
        };
        let availability_time_offset = base
            .offset
            .checked_add(built.offset)
            .ok_or(ManifestError::overflow("availabilityTimeOffset"))?;

        let codecs = common.codecs.as_ref().or(set.common.codecs.as_ref());
        self.charge(
            mime_type.len()
                + codecs.map_or(0, String::len)
                + base.url.as_str().len()
                + built.charged_bytes,
        )?;
        Ok(Representation {
            id: representation.id,
            bandwidth: representation.bandwidth,
            content_type,
            mime_type: mime_type.clone(),
            codecs: codecs.cloned(),
            width: common.width.or(set.common.width),
            height: common.height.or(set.common.height),
            start_with_sap: common.start_with_sap.or(set.common.start_with_sap),
            availability_time_offset,
            availability_time_complete: base.complete && built.complete,
            initialization: built.initialization,
            addressing: built.addressing,
            segment_index: built.segment_index,
            timing: context.timing,
        })
    }

    fn build_addressing(
        &mut self,
        info: &MultipleSegmentInfo,
        representation_id: &str,
        bandwidth: u64,
        base: &Url,
    ) -> Result<BuiltAddressing, ManifestError> {
        let (common, list) = match info {
            MultipleSegmentInfo::Template(template) => (&template.base, None),
            MultipleSegmentInfo::List(list) => (&list.base, Some(list)),
        };
        if common.ept_delta.is_some_and(|delta| delta != 0) {
            return Err(UnsupportedFeature::EptDelta.into());
        }
        let offset = availability_offset(common.availability_time_offset)?;
        let timescale = common.timescale.unwrap_or(1);
        let presentation_time_offset = common.presentation_time_offset.unwrap_or(0);
        let start_number = common.start_number.unwrap_or(1);
        if common.end_number.is_some_and(|end| end < start_number) {
            return Err(ManifestError::inconsistent(
                "@endNumber is below @startNumber",
            ));
        }

        let (runs, has_timeline): (Arc<[Run]>, bool) = match (common.duration, &common.timeline) {
            (Some(_), Some(_)) => {
                return Err(ManifestError::inconsistent(
                    "both @duration and SegmentTimeline apply to a Representation",
                ));
            }
            // Simple addressing (DASH-IF timing model §18.4): segment k starts
            // k·@duration after the Period start, i.e. at media time
            // PTO + k·@duration.
            (Some(duration), None) => {
                let count = list
                    .map(|list| u64::try_from(list.segment_urls.len()))
                    .transpose()
                    .map_err(|_| ManifestError::overflow("SegmentList length"))?;
                let run = Run {
                    start: presentation_time_offset,
                    duration,
                    count,
                    first_index: 0,
                };
                (Arc::from([run]), false)
            }
            (None, Some(entries)) => {
                if entries.iter().any(|entry| entry.has_n) {
                    return Err(UnsupportedFeature::TimelineAttribute("n").into());
                }
                if entries.iter().any(|entry| entry.k.is_some_and(|k| k != 1)) {
                    return Err(UnsupportedFeature::TimelineAttribute("k").into());
                }
                let runs = self.timeline_runs(entries)?;
                if let Some(list) = list {
                    check_list_cardinality(&runs, list.segment_urls.len())?;
                }
                (runs, true)
            }
            (None, None) => return Err(UnsupportedFeature::UntimedSegments.into()),
        };

        let join = |reference: &str, element: &'static str| {
            base.join(reference)
                .map_err(|source| ManifestError::InvalidUrl { element, source })
        };
        let init_from_element = || {
            common
                .initialization
                .as_ref()
                .map(|init| init_segment(init, base))
                .transpose()
        };

        let mut charged_bytes = 0;
        let (urls, initialization) = match info {
            MultipleSegmentInfo::Template(template) => {
                let media = template
                    .media
                    .as_deref()
                    .ok_or(ManifestError::MissingAttribute {
                        element: "SegmentTemplate",
                        attribute: "media",
                    })?;
                let media =
                    Template::compile(media, TemplateKind::Media, representation_id, bandwidth)?;
                // Catch URLs that can never resolve before any segment is planned.
                let first = media.expand(start_number, presentation_time_offset);
                charged_bytes += first.len();
                join(&first, "SegmentTemplate@media")?;
                let initialization = match &template.initialization {
                    Some(init) => {
                        let init = Template::compile(
                            init,
                            TemplateKind::Initialization,
                            representation_id,
                            bandwidth,
                        )?;
                        Some(InitSegment {
                            url: join(&init.expand(0, 0), "SegmentTemplate@initialization")?,
                            byte_range: None,
                        })
                    }
                    None => init_from_element()?,
                };
                (
                    SegmentUrls::Template {
                        media,
                        base: base.clone(),
                    },
                    initialization,
                )
            }
            MultipleSegmentInfo::List(list) => {
                // Entries are shared and resolved per segment; resolving the
                // first one rejects an unusable base early.
                if let Some(first) = list.segment_urls.first()
                    && let Some(media) = &first.media
                {
                    join(media, "SegmentURL@media")?;
                }
                (
                    SegmentUrls::List {
                        urls: Arc::clone(&list.segment_urls),
                        base: base.clone(),
                    },
                    init_from_element()?,
                )
            }
        };
        charged_bytes += base.as_str().len()
            + initialization
                .as_ref()
                .map_or(0, |init| init.url.as_str().len());

        Ok(BuiltAddressing {
            addressing: Addressing {
                timescale,
                presentation_time_offset,
                start_number,
                end_number: common.end_number,
                has_timeline,
                runs,
                urls,
            },
            initialization,
            segment_index: None,
            offset,
            complete: common.availability_time_complete.unwrap_or(true),
            charged_bytes,
        })
    }
}

/// An `availabilityTimeOffset` as given, zero when absent.
fn availability_offset(raw: Option<RawOffset>) -> Result<AvailabilityTimeOffset, ManifestError> {
    match raw {
        None => Ok(AvailabilityTimeOffset::ZERO),
        Some(RawOffset::Offset(offset)) => Ok(offset),
        Some(RawOffset::Negative) => Err(UnsupportedFeature::NegativeAvailabilityTimeOffset.into()),
    }
}

/// An `Initialization` element resolved against `base`. Without
/// `@sourceURL` the BaseURL itself is the resource (`URLType`, ISO/IEC
/// 23009-1 §5.3.9.2).
fn init_segment(init: &RawUrl, base: &Url) -> Result<InitSegment, ManifestError> {
    let url = match &init.source_url {
        Some(source) => base
            .join(source)
            .map_err(|source| ManifestError::InvalidUrl {
                element: "Initialization@sourceURL",
                source,
            })?,
        None => base.clone(),
    };
    Ok(InitSegment {
        url,
        byte_range: init.range,
    })
}

/// Addressing of a `SegmentBase` Representation, whose resource is `url`,
/// pending its Segment Index.
fn build_indexed(base: &RawSegmentBase, url: &Url) -> Result<BuiltAddressing, ManifestError> {
    if base.ept_delta.is_some_and(|delta| delta != 0) {
        return Err(UnsupportedFeature::EptDelta.into());
    }
    let offset = availability_offset(base.availability_time_offset)?;
    // Where both are given, which index applies is not defined; the
    // resource may not carry one at all.
    if base.has_representation_index {
        return Err(UnsupportedFeature::RepresentationIndex.into());
    }
    let Some(index_range) = base.index_range else {
        return Err(UnsupportedFeature::UnindexedResource.into());
    };
    // Without `Initialization` the resource is self-initializing: its
    // initialization data (`ftyp`, `moov`) precedes the Segment Index, as in
    // the indexed layout of the on-demand profile. Where the index box
    // starts is known once it is read.
    let initialization = base
        .initialization
        .as_ref()
        .map(|init| init_segment(init, url))
        .transpose()?;
    let charged_bytes = url.as_str().len()
        + initialization
            .as_ref()
            .map_or(0, |init| init.url.as_str().len());
    Ok(BuiltAddressing {
        // Placeholder timing in the MPD's timescale until the index is
        // applied (`indexed_addressing`).
        addressing: Addressing {
            timescale: base.timescale.unwrap_or(1),
            presentation_time_offset: base.presentation_time_offset.unwrap_or(0),
            start_number: 1,
            end_number: None,
            has_timeline: true,
            runs: Arc::from([]),
            urls: SegmentUrls::Indexed {
                url: url.clone(),
                ranges: Arc::from([]),
            },
        },
        segment_index: Some(PendingIndex {
            location: SegmentIndexLocation {
                url: url.clone(),
                range: index_range,
            },
            implicit_initialization: initialization.is_none(),
            unreadable: None,
        }),
        initialization,
        offset,
        complete: base.availability_time_complete.unwrap_or(true),
        charged_bytes,
    })
}

/// Rejects MPD chaining and `EssentialProperty` schemes this engine does not
/// understand at MPD or Period level.
fn check_properties(properties: &[RawProperty]) -> Result<(), ManifestError> {
    for property in properties {
        let scheme = property.scheme_id_uri.as_str();
        if CHAINING_SCHEMES.contains(&scheme) {
            return Err(UnsupportedFeature::MpdChaining(scheme.to_owned()).into());
        }
        if property.essential && !UNDERSTOOD_ESSENTIAL_PROPERTIES.contains(&scheme) {
            return Err(UnsupportedFeature::EssentialProperty(scheme.to_owned()).into());
        }
    }
    Ok(())
}

/// A `SegmentList` with a `SegmentTimeline` describes one segment per
/// `SegmentURL`: the counts must agree, and a trailing `S@r=-1` repeats for
/// the remaining URLs.
fn check_list_cardinality(runs: &[Run], urls: usize) -> Result<(), ManifestError> {
    let urls = u64::try_from(urls).map_err(|_| ManifestError::overflow("SegmentList length"))?;
    let (listed, open) = match runs.last() {
        None => (0, false),
        Some(last) => match last.count {
            Some(count) => (
                last.first_index
                    .checked_add(count)
                    .ok_or(ManifestError::overflow("SegmentTimeline"))?,
                false,
            ),
            None => (last.first_index, true),
        },
    };
    let consistent = if open { urls > listed } else { urls == listed };
    if !consistent {
        return Err(ManifestError::inconsistent(format!(
            "SegmentList has {urls} SegmentURLs but its SegmentTimeline describes {}{listed}",
            if open { "more than " } else { "" }
        )));
    }
    Ok(())
}

struct SetContext<'a> {
    set: &'a RawAdaptationSet,
    period_info: &'a RawSegmentInfo,
    timing: PeriodTiming,
}

fn check_set(set: &RawAdaptationSet, period_base: &BaseChain) -> Result<BaseChain, ManifestError> {
    if set.has_xlink {
        return Err(UnsupportedFeature::XLink {
            element: "AdaptationSet",
        }
        .into());
    }
    check_descriptors(&set.common)?;
    period_base.join(set.base_url.as_ref())
}

fn check_descriptors(common: &RawCommon) -> Result<(), ManifestError> {
    if common.has_content_protection {
        return Err(UnsupportedFeature::ContentProtection.into());
    }
    if let Some(scheme) = common
        .essential_properties
        .iter()
        .find(|scheme| !UNDERSTOOD_ESSENTIAL_PROPERTIES.contains(&scheme.as_str()))
    {
        return Err(UnsupportedFeature::EssentialProperty(scheme.clone()).into());
    }
    Ok(())
}

/// The resolved BaseURL and the availability attributes of the BaseURLs that
/// contributed to it.
#[derive(Debug, Clone)]
struct BaseChain {
    url: Url,
    offset: AvailabilityTimeOffset,
    complete: bool,
}

impl BaseChain {
    fn new(document_url: Url) -> Self {
        Self {
            url: document_url,
            offset: AvailabilityTimeOffset::ZERO,
            complete: true,
        }
    }

    /// Applies one level's (first) `BaseURL` (ISO/IEC 23009-1 §5.6). An
    /// absolute URL replaces the chain, so the offsets of outer BaseURLs no
    /// longer apply.
    fn join(&self, base_url: Option<&RawBaseUrl>) -> Result<Self, ManifestError> {
        let Some(base_url) = base_url else {
            return Ok(self.clone());
        };
        if base_url.has_byte_range {
            return Err(UnsupportedFeature::BaseUrlByteRange.into());
        }
        let offset = availability_offset(base_url.availability_time_offset)?;
        let url = self
            .url
            .join(&base_url.url)
            .map_err(|source| ManifestError::InvalidUrl {
                element: "BaseURL",
                source,
            })?;
        if url.as_str().len() > MAX_URL_BYTES {
            return Err(ManifestError::LimitExceeded {
                what: "resolved BaseURL",
                limit: MAX_URL_BYTES,
            });
        }
        let complete = base_url.availability_time_complete.unwrap_or(true);
        if has_scheme(&base_url.url) {
            return Ok(Self {
                url,
                offset,
                complete,
            });
        }
        Ok(Self {
            url,
            offset: self
                .offset
                .checked_add(offset)
                .ok_or(ManifestError::overflow("availabilityTimeOffset"))?,
            complete: self.complete && complete,
        })
    }
}

/// Whether `reference` is an absolute URI (RFC 3986 §4.3: it has a scheme).
fn has_scheme(reference: &str) -> bool {
    let Some((scheme, _)) = reference.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Segment information after inheritance.
enum SegmentInfo {
    Multiple(MultipleSegmentInfo),
    Base(RawSegmentBase),
}

/// Segment information listing several segments (`MultipleSegmentBaseType`).
enum MultipleSegmentInfo {
    Template(RawSegmentTemplate),
    List(RawSegmentList),
}

/// Merges `SegmentTemplate`, `SegmentList` or `SegmentBase` from Period →
/// AdaptationSet → Representation: each attribute and child of a lower level
/// overrides the same one above it (ISO/IEC 23009-1 §5.3.9.1). Child lists
/// are shared, not copied.
///
/// Each level carries at most one addressing element (checked while
/// parsing). The innermost level carrying one decides the addressing type,
/// and only elements of that type are merged. Without any, the
/// Representation is a single resource with no index, which is out of scope.
/// `SegmentTemplate` and `SegmentList` in one chain are contradictory.
fn merge_segment_info(levels: [&RawSegmentInfo; 3]) -> Result<SegmentInfo, ManifestError> {
    let innermost = levels
        .iter()
        .rev()
        .find(|level| level.template.is_some() || level.list.is_some() || level.base.is_some());
    let Some(innermost) = innermost else {
        return Err(UnsupportedFeature::UnindexedResource.into());
    };
    if innermost.base.is_some() {
        let mut merged = RawSegmentBase::default();
        for base in levels.iter().filter_map(|level| level.base.as_ref()) {
            merged.override_with(base);
        }
        return Ok(SegmentInfo::Base(merged));
    }
    let templates: Vec<&RawSegmentTemplate> = levels
        .iter()
        .filter_map(|level| level.template.as_ref())
        .collect();
    let lists: Vec<&RawSegmentList> = levels
        .iter()
        .filter_map(|level| level.list.as_ref())
        .collect();
    if !templates.is_empty() && !lists.is_empty() {
        return Err(ManifestError::inconsistent(
            "both SegmentTemplate and SegmentList apply to a Representation",
        ));
    }
    if innermost.template.is_some() {
        let mut merged = RawSegmentTemplate::default();
        for template in templates {
            merged.base.override_with(&template.base);
            if template.media.is_some() {
                merged.media.clone_from(&template.media);
            }
            if template.initialization.is_some() {
                merged.initialization.clone_from(&template.initialization);
            }
        }
        Ok(SegmentInfo::Multiple(MultipleSegmentInfo::Template(merged)))
    } else {
        let mut merged = RawSegmentList::default();
        for list in lists {
            if list.has_xlink {
                return Err(UnsupportedFeature::XLink {
                    element: "SegmentList",
                }
                .into());
            }
            merged.base.override_with(&list.base);
            if !list.segment_urls.is_empty() {
                merged.segment_urls = Arc::clone(&list.segment_urls);
            }
        }
        Ok(SegmentInfo::Multiple(MultipleSegmentInfo::List(merged)))
    }
}

/// Overrides one attribute or child with an inner level's.
fn take<T: Clone>(outer: &mut Option<T>, inner: &Option<T>) {
    if inner.is_some() {
        outer.clone_from(inner);
    }
}

impl RawSegmentBase {
    fn override_with(&mut self, inner: &Self) {
        take(&mut self.timescale, &inner.timescale);
        take(
            &mut self.presentation_time_offset,
            &inner.presentation_time_offset,
        );
        take(&mut self.ept_delta, &inner.ept_delta);
        take(&mut self.index_range, &inner.index_range);
        take(
            &mut self.availability_time_offset,
            &inner.availability_time_offset,
        );
        take(
            &mut self.availability_time_complete,
            &inner.availability_time_complete,
        );
        take(&mut self.initialization, &inner.initialization);
        self.has_representation_index |= inner.has_representation_index;
    }
}

impl RawMultipleSegmentBase {
    fn override_with(&mut self, inner: &Self) {
        take(&mut self.timescale, &inner.timescale);
        take(
            &mut self.presentation_time_offset,
            &inner.presentation_time_offset,
        );
        take(&mut self.duration, &inner.duration);
        take(&mut self.start_number, &inner.start_number);
        take(&mut self.end_number, &inner.end_number);
        take(&mut self.ept_delta, &inner.ept_delta);
        take(
            &mut self.availability_time_offset,
            &inner.availability_time_offset,
        );
        take(
            &mut self.availability_time_complete,
            &inner.availability_time_complete,
        );
        take(&mut self.timeline, &inner.timeline);
        take(&mut self.initialization, &inner.initialization);
    }
}

/// A `SegmentBase` Representation's unread Segment Index.
#[derive(Debug, Clone)]
struct PendingIndex {
    location: SegmentIndexLocation,
    /// No `Initialization` element: the initialization data is everything
    /// before the index box.
    implicit_initialization: bool,
    /// Why reading it failed, when it was tried.
    unreadable: Option<SharedError>,
}

struct BuiltAddressing {
    addressing: Addressing,
    initialization: Option<InitSegment>,
    segment_index: Option<PendingIndex>,
    offset: AvailabilityTimeOffset,
    complete: bool,
    /// Bytes of URLs and templates created for the Representation.
    charged_bytes: usize,
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn url() -> Url {
        Url::parse("https://origin.example.com/live/stream.mpd?token=secret").unwrap()
    }

    fn manifest(xml: &str) -> Manifest {
        Manifest::parse(xml.as_bytes(), &url()).unwrap()
    }

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn segment_template_attributes_inherit_per_attribute() {
        let manifest = manifest(
            r#"<MPD type="static" mediaPresentationDuration="PT8S">
  <Period>
    <SegmentTemplate timescale="1000" duration="2000" startNumber="7"
                     media="$RepresentationID$/$Number%03d$.m4s"/>
    <AdaptationSet mimeType="video/mp4" codecs="avc1.64001f" width="1280" height="720">
      <SegmentTemplate initialization="$RepresentationID$/init-$Bandwidth$.mp4"/>
      <Representation id="v1" bandwidth="800000"/>
      <Representation id="v2" bandwidth="1600000" height="1080">
        <SegmentTemplate startNumber="1"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#,
        );
        let set = &manifest.periods[0].adaptation_sets[0];
        let v1 = &set.representations[0];
        assert_eq!(v1.codecs.as_deref(), Some("avc1.64001f"));
        assert_eq!((v1.width, v1.height), (Some(1280), Some(720)));
        assert_eq!(
            v1.initialization.as_ref().unwrap().url.as_str(),
            "https://origin.example.com/live/v1/init-800000.mp4"
        );
        let numbers: Vec<_> = v1
            .segments(at(0))
            .unwrap()
            .iter()
            .map(|s| s.unwrap().number)
            .collect();
        assert_eq!(numbers, [7, 8, 9, 10]);
        let first = v1.segments(at(0)).unwrap().first().unwrap().unwrap();
        assert_eq!(
            first.url.as_str(),
            "https://origin.example.com/live/v1/007.m4s"
        );

        let v2 = &set.representations[1];
        assert_eq!(v2.height, Some(1080));
        assert_eq!(v2.segments(at(0)).unwrap().numbers(), 1..5);
    }

    #[test]
    fn base_url_chain_resolves_and_absolute_urls_reset_it() {
        let manifest = manifest(
            r#"<MPD type="static" mediaPresentationDuration="PT2S">
  <BaseURL availabilityTimeOffset="1">cdn/</BaseURL>
  <Period>
    <BaseURL availabilityTimeOffset="2">period/</BaseURL>
    <AdaptationSet mimeType="audio/mp4">
      <BaseURL>audio/</BaseURL>
      <SegmentTemplate duration="2" media="$Number$.m4s" initialization="init.mp4"/>
      <Representation id="a1" bandwidth="1">
        <BaseURL>a1/</BaseURL>
      </Representation>
      <Representation id="a2" bandwidth="1">
        <BaseURL availabilityTimeOffset="0.5">https://other.example.com/a2/</BaseURL>
      </Representation>
      <Representation id="a3" bandwidth="1">
        <BaseURL>/root/</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#,
        );
        let set = &manifest.periods[0].adaptation_sets[0];
        let a1 = &set.representations[0];
        assert_eq!(a1.content_type, ContentType::Audio);
        assert_eq!(
            a1.initialization.as_ref().unwrap().url.as_str(),
            "https://origin.example.com/live/cdn/period/audio/a1/init.mp4"
        );
        assert_eq!(
            a1.availability_time_offset,
            AvailabilityTimeOffset::Finite(Duration::from_secs(3))
        );
        let a2 = &set.representations[1];
        assert_eq!(
            a2.segments(at(0))
                .unwrap()
                .first()
                .unwrap()
                .unwrap()
                .url
                .as_str(),
            "https://other.example.com/a2/1.m4s"
        );
        assert_eq!(
            a2.availability_time_offset,
            AvailabilityTimeOffset::Finite(Duration::from_millis(500))
        );
        let a3 = &set.representations[2];
        assert_eq!(
            a3.initialization.as_ref().unwrap().url.as_str(),
            "https://origin.example.com/root/init.mp4"
        );
    }

    #[test]
    fn availability_time_offset_sums_base_url_and_segment_information() {
        let manifest = manifest(
            r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z">
  <BaseURL availabilityTimeOffset="0.5">https://cdn.example.com/</BaseURL>
  <Period start="PT0S">
    <SegmentTemplate availabilityTimeOffset="5"/>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate duration="2" media="$Number$.m4s" availabilityTimeOffset="1.5"/>
      <Representation id="v" bandwidth="1"/>
      <Representation id="low-latency" bandwidth="1">
        <SegmentTemplate availabilityTimeComplete="false"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#,
        );
        let set = &manifest.periods[0].adaptation_sets[0];
        let full = &set.representations[0];
        // BaseURL 0.5 + innermost segment information 1.5 (overrides the
        // Period's 5).
        assert_eq!(
            full.availability_time_offset,
            AvailabilityTimeOffset::Finite(Duration::from_secs(2))
        );
        assert!(full.availability_time_complete);
        // At x = 10 with A = 2 the segment ending at 12 is available.
        assert_eq!(full.segments(at(10)).unwrap().numbers(), 1..7);

        let chunked = &set.representations[1];
        assert!(!chunked.availability_time_complete);
        assert_eq!(chunked.segments(at(10)).unwrap().numbers(), 1..6);
    }

    #[test]
    fn period_timing_is_derived() {
        let manifest = manifest(
            r#"<MPD type="static" mediaPresentationDuration="PT60S">
  <Period id="a" duration="PT10S"/>
  <Period id="b" duration="PT20S"/>
  <Period id="c" start="PT40S"/>
</MPD>"#,
        );
        let timing: Vec<_> = manifest
            .periods
            .iter()
            .map(|p| {
                (
                    p.id.as_deref().unwrap(),
                    p.start.as_secs(),
                    p.duration.map(|d| d.as_secs()),
                )
            })
            .collect();
        assert_eq!(
            timing,
            [("a", 0, Some(10)), ("b", 10, Some(30)), ("c", 40, Some(20))]
        );
    }

    #[test]
    fn dynamic_periods_without_derivable_start_are_early_available() {
        let manifest = manifest(
            r#"<MPD type="dynamic" availabilityStartTime="2024-01-01T00:00:00Z">
  <Period id="live" start="PT100S"/>
  <Period id="next"/>
</MPD>"#,
        );
        assert_eq!(manifest.periods.len(), 1);
        assert_eq!(manifest.periods[0].duration, None);
        assert_eq!(manifest.early_available_periods, 1);
    }

    #[test]
    fn inconsistent_periods_are_rejected() {
        for xml in [
            r#"<MPD type="static"><Period start="PT10S"/><Period start="PT5S"/></MPD>"#,
            r#"<MPD type="static"><Period/><Period/></MPD>"#,
            r#"<MPD type="static" mediaPresentationDuration="PT5S"><Period start="PT10S"/></MPD>"#,
            r#"<MPD type="dynamic"><Period start="PT0S"/></MPD>"#,
        ] {
            assert!(Manifest::parse(xml.as_bytes(), &url()).is_err(), "{xml}");
        }
    }

    #[test]
    fn out_of_scope_content_is_rejected_per_element() {
        let manifest = manifest(
            r#"<MPD type="static" mediaPresentationDuration="PT2S"
     xmlns:xlink="http://www.w3.org/1999/xlink">
  <Period>
    <SegmentTemplate duration="2" media="$Number$.m4s"/>
    <AdaptationSet id="1" mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <Representation id="drm" bandwidth="1"/>
    </AdaptationSet>
    <AdaptationSet id="2" mimeType="video/webm">
      <Representation id="webm" bandwidth="1"/>
    </AdaptationSet>
    <AdaptationSet id="3" mimeType="video/mp4">
      <Representation id="unindexed" bandwidth="1">
        <SegmentBase/>
      </Representation>
      <Representation id="clear" bandwidth="1"/>
      <Representation id="rep-drm" bandwidth="1">
        <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet id="4" xlink:href="https://ads.example.com/set.xml"/>
    <AdaptationSet id="5" mimeType="video/mp4">
      <EssentialProperty schemeIdUri="http://dashif.org/guidelines/trickmode" value="3"/>
      <Representation id="trick" bandwidth="1"/>
    </AdaptationSet>
    <AdaptationSet id="6" mimeType="video/mp4">
      <EssentialProperty schemeIdUri="urn:mpeg:mpegB:cicp:TransferCharacteristics" value="16"/>
      <Representation id="hdr" bandwidth="1"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
        );
        let period = &manifest.periods[0];
        let usable: Vec<_> = period
            .adaptation_sets
            .iter()
            .flat_map(|set| set.representations.iter().map(|r| r.id.as_str()))
            .collect();
        assert_eq!(usable, ["clear", "hdr"]);
        let rejected: Vec<_> = period
            .rejected
            .iter()
            .map(|r| {
                (
                    r.adaptation_set_id.as_deref(),
                    r.representation_id.as_deref(),
                    r.feature.clone(),
                )
            })
            .collect();
        assert_eq!(
            rejected,
            [
                (Some("1"), None, UnsupportedFeature::ContentProtection),
                (
                    Some("2"),
                    Some("webm"),
                    UnsupportedFeature::MimeType("video/webm".into())
                ),
                (
                    Some("3"),
                    Some("unindexed"),
                    UnsupportedFeature::UnindexedResource
                ),
                (
                    Some("3"),
                    Some("rep-drm"),
                    UnsupportedFeature::ContentProtection
                ),
                (
                    Some("4"),
                    None,
                    UnsupportedFeature::XLink {
                        element: "AdaptationSet"
                    }
                ),
                (
                    Some("5"),
                    None,
                    UnsupportedFeature::EssentialProperty(
                        "http://dashif.org/guidelines/trickmode".into()
                    )
                ),
            ]
        );
        assert!(period.ensure_usable().is_ok());
    }

    #[test]
    fn drm_only_period_is_unusable() {
        let manifest = manifest(
            r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
  <AdaptationSet mimeType="video/mp4">
    <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
    <SegmentTemplate duration="2" media="$Number$.m4s"/>
    <Representation id="v" bandwidth="1"/>
  </AdaptationSet>
</Period></MPD>"#,
        );
        let error = manifest.periods[0].ensure_usable().unwrap_err();
        assert!(matches!(
            error,
            ManifestError::Unsupported(UnsupportedFeature::ContentProtection)
        ));
        let download: crate::DownloadError = error.into();
        assert!(matches!(download, crate::DownloadError::Unsupported { .. }));
    }

    #[test]
    fn xlink_period_is_unsupported() {
        let error = Manifest::parse(
            br#"<MPD xmlns:xlink="http://www.w3.org/1999/xlink"><Period xlink:href="https://x/p.xml"/></MPD>"#,
            &url(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::Unsupported(UnsupportedFeature::XLink { element: "Period" })
        ));
    }

    #[test]
    fn malformed_segment_information_fails_the_manifest() {
        let wrap = |inner: &str| {
            format!(
                r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}<Representation id="v" bandwidth="1"/></AdaptationSet>
</Period></MPD>"#
            )
        };
        for inner in [
            r#"<SegmentTemplate duration="2"/>"#,
            r#"<SegmentTemplate duration="2" media="seg.m4s"/>"#,
            r#"<SegmentTemplate duration="2" media="$Unknown$.m4s"/>"#,
            r#"<SegmentTemplate duration="2" media="$Number$.m4s"><SegmentTimeline><S d="2"/></SegmentTimeline></SegmentTemplate>"#,
            r#"<SegmentTemplate duration="2" media="$Number$.m4s" initialization="$Number$.mp4"/>"#,
            r#"<SegmentTemplate duration="2" media="http://[bad/$Number$.m4s"/>"#,
            r#"<SegmentTemplate duration="2" media="$Number$"/><SegmentList duration="2"/>"#,
        ] {
            let xml = wrap(inner);
            let result = Manifest::parse(xml.as_bytes(), &url());
            assert!(
                matches!(&result, Err(error) if !matches!(error, ManifestError::Unsupported(_))),
                "{inner}: {result:?}"
            );
        }
        let xml = r#"<MPD><Period><AdaptationSet><Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::MissingAttribute {
                attribute: "mimeType",
                ..
            })
        ));
        let duplicate = r#"<MPD><Period><AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"/></AdaptationSet><AdaptationSet mimeType="audio/mp4"><Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
        assert!(Manifest::parse(duplicate.as_bytes(), &url()).is_err());
    }

    #[test]
    fn unsupported_segment_information_is_rejected() {
        let wrap = |inner: &str| {
            format!(
                r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}<Representation id="v" bandwidth="1"/></AdaptationSet>
</Period></MPD>"#
            )
        };
        let cases = [
            (
                r#"<SegmentTemplate media="$Number$.m4s"/>"#,
                UnsupportedFeature::UntimedSegments,
            ),
            (
                r#"<SegmentTemplate duration="2" eptDelta="-5" media="$Number$.m4s"/>"#,
                UnsupportedFeature::EptDelta,
            ),
            (
                r#"<SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" n="4"/></SegmentTimeline></SegmentTemplate>"#,
                UnsupportedFeature::TimelineAttribute("n"),
            ),
            (
                r#"<SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" k="2"/></SegmentTimeline></SegmentTemplate>"#,
                UnsupportedFeature::TimelineAttribute("k"),
            ),
            (
                r#"<SegmentTemplate duration="2" media="$Number$-$SubNumber$.m4s"/>"#,
                UnsupportedFeature::TemplateIdentifier("SubNumber".into()),
            ),
            (
                r#"<BaseURL byteRange="$base$?r=$first$">a/</BaseURL><SegmentTemplate duration="2" media="$Number$"/>"#,
                UnsupportedFeature::BaseUrlByteRange,
            ),
            ("", UnsupportedFeature::UnindexedResource),
            (
                r#"<SegmentBase><RepresentationIndex sourceURL="index.sidx"/></SegmentBase>"#,
                UnsupportedFeature::RepresentationIndex,
            ),
            (
                r#"<SegmentBase indexRange="10-20"><RepresentationIndex sourceURL="index.sidx"/></SegmentBase>"#,
                UnsupportedFeature::RepresentationIndex,
            ),
            (
                r#"<SegmentBase indexRange="10-20" eptDelta="3"/>"#,
                UnsupportedFeature::EptDelta,
            ),
        ];
        for (inner, expected) in cases {
            let manifest = manifest(&wrap(inner));
            let period = &manifest.periods[0];
            assert!(period.adaptation_sets.is_empty(), "{inner}");
            assert_eq!(period.rejected[0].feature, expected, "{inner}");
        }
    }

    #[test]
    fn utc_timing_and_location() {
        let manifest = manifest(
            r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z">
  <Location>next.mpd</Location>
  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:ntp:2014" value="pool.ntp.org"/>
  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:http-xsdate:2014" value="/time"/>
</MPD>"#,
        );
        assert_eq!(
            manifest.location.as_ref().map(Url::as_str),
            Some("https://origin.example.com/live/next.mpd")
        );
        assert_eq!(
            manifest.utc_timings,
            [UtcTiming::HttpXsDate {
                urls: vec![Url::parse("https://origin.example.com/time").unwrap()]
            }]
        );
    }

    #[test]
    fn has_scheme_follows_rfc_3986() {
        assert!(has_scheme("https://a/"));
        assert!(has_scheme("a+b.c-d:x"));
        assert!(!has_scheme("/path:with:colons"));
        assert!(!has_scheme("relative/seg.m4s"));
        assert!(!has_scheme("1http://a/"));
        assert!(!has_scheme(""));
    }

    fn wrap_set(inner: &str) -> String {
        format!(
            r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}</AdaptationSet>
</Period></MPD>"#
        )
    }

    #[test]
    fn inherited_timelines_and_lists_are_shared_not_copied() {
        let parsed = manifest(&wrap_set(
            r#"<SegmentTemplate media="$Time$.m4s"><SegmentTimeline><S t="0" d="2" r="3"/></SegmentTimeline></SegmentTemplate>
<Representation id="a" bandwidth="1"/><Representation id="b" bandwidth="2"/>"#,
        ));
        let reps = &parsed.periods[0].adaptation_sets[0].representations;
        assert!(Arc::ptr_eq(
            &reps[0].addressing.runs,
            &reps[1].addressing.runs
        ));

        let parsed = manifest(&wrap_set(
            r#"<SegmentList duration="2"><SegmentURL media="1.m4s"/><SegmentURL media="2.m4s"/></SegmentList>
<Representation id="a" bandwidth="1"><BaseURL>a/</BaseURL></Representation>
<Representation id="b" bandwidth="2"><BaseURL>b/</BaseURL><SegmentList startNumber="5"/></Representation>"#,
        ));
        let reps = &parsed.periods[0].adaptation_sets[0].representations;
        let (SegmentUrls::List { urls: a, .. }, SegmentUrls::List { urls: b, .. }) =
            (&reps[0].addressing.urls, &reps[1].addressing.urls)
        else {
            panic!("expected list addressing");
        };
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(reps[1].addressing_kind(), AddressingKind::List);
        let b_segments: Vec<_> = reps[1]
            .segments(at(0))
            .unwrap()
            .iter()
            .map(|segment| {
                let segment = segment.unwrap();
                (segment.number, segment.url.to_string())
            })
            .collect();
        assert_eq!(
            b_segments,
            [
                (5, "https://origin.example.com/live/b/1.m4s".to_owned()),
                (6, "https://origin.example.com/live/b/2.m4s".to_owned()),
            ]
        );
    }

    #[test]
    fn hostile_amplification_is_rejected_cheaply() {
        // Inherited strings multiplied by many Representations exceed the
        // aggregate budget long before 10 000 × 4 KiB × several copies.
        let codecs = "a".repeat(4_000);
        let base = format!("https://cdn.example.com/{}/", "p".repeat(30_000));
        let reps: String = (0..10_000)
            .map(|i| format!(r#"<Representation id="{i}" bandwidth="1"/>"#))
            .collect();
        let xml = format!(
            r#"<MPD type="static" mediaPresentationDuration="PT2S"><BaseURL>{base}</BaseURL><Period>
<AdaptationSet mimeType="video/mp4" codecs="{codecs}"><SegmentTemplate duration="2" media="$RepresentationID$/$Number$"/>{reps}</AdaptationSet>
</Period></MPD>"#
        );
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::LimitExceeded {
                what: "resolved MPD data",
                ..
            })
        ));

        // Over-long identifiers and templates are refused while parsing.
        let long_id = "x".repeat(4 * 1024 * 1024);
        let xml = wrap_set(&format!(
            r#"<SegmentTemplate duration="2" media="{}$Number$"/><Representation id="{long_id}" bandwidth="1"/>"#,
            "$RepresentationID$".repeat(200_000)
        ));
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::LimitExceeded { .. })
        ));
        let id = "x".repeat(4_000);
        let xml = wrap_set(&format!(
            r#"<SegmentTemplate duration="2" media="{}$Number$"/><Representation id="{id}" bandwidth="1"/>"#,
            "$RepresentationID$".repeat(1_000)
        ));
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::LimitExceeded {
                what: "expanded URL template",
                ..
            })
        ));
    }

    #[test]
    fn element_counts_are_bounded() {
        use super::super::limits::{MAX_PERIODS, MAX_REPRESENTATIONS, MAX_TIMELINE_ENTRIES};
        let periods = r#"<Period duration="PT1S"/>"#.repeat(MAX_PERIODS + 1);
        let xml = format!(r#"<MPD type="static">{periods}</MPD>"#);
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::LimitExceeded { .. })
        ));
        let reps: String = (0..=MAX_REPRESENTATIONS)
            .map(|i| format!(r#"<Representation id="{i}" bandwidth="1"/>"#))
            .collect();
        assert!(matches!(
            Manifest::parse(wrap_set(&reps).as_bytes(), &url()),
            Err(ManifestError::LimitExceeded { .. })
        ));
        let entries = r#"<S d="1"/>"#.repeat(MAX_TIMELINE_ENTRIES + 1);
        let xml = wrap_set(&format!(
            r#"<SegmentTemplate media="$Time$"><SegmentTimeline>{entries}</SegmentTimeline></SegmentTemplate><Representation id="v" bandwidth="1"/>"#
        ));
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn negative_availability_time_offset_is_unsupported() {
        let manifest = manifest(&wrap_set(
            r#"<SegmentTemplate duration="2" media="$Number$"/>
<Representation id="negative" bandwidth="1"><SegmentTemplate availabilityTimeOffset="-1"/></Representation>
<Representation id="base" bandwidth="1"><BaseURL availabilityTimeOffset="-INF">x/</BaseURL></Representation>
<Representation id="fine" bandwidth="1"/>"#,
        ));
        let period = &manifest.periods[0];
        assert_eq!(period.adaptation_sets[0].representations.len(), 1);
        let features: Vec<_> = period.rejected.iter().map(|r| r.feature.clone()).collect();
        assert_eq!(
            features,
            [
                UnsupportedFeature::NegativeAvailabilityTimeOffset,
                UnsupportedFeature::NegativeAvailabilityTimeOffset,
            ]
        );
    }

    #[test]
    fn contradictory_content_type_is_malformed() {
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet contentType="audio" mimeType="video/mp4"><SegmentTemplate duration="2" media="$Number$"/>
<Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::Inconsistent { .. })
        ));
        let xml = xml.replace(r#"contentType="audio""#, r#"contentType="text""#);
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::Inconsistent { .. })
        ));
        let xml = xml.replace(r#"contentType="text""#, r#"contentType="video""#);
        assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
    }

    #[test]
    fn duplicate_ids_are_malformed() {
        for xml in [
            r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period id="a" duration="PT2S"/><Period id="a"/></MPD>"#,
            r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period><AdaptationSet id="1"/><AdaptationSet id="1"/></Period></MPD>"#,
        ] {
            assert!(
                matches!(
                    Manifest::parse(xml.as_bytes(), &url()),
                    Err(ManifestError::Inconsistent { .. })
                ),
                "{xml}"
            );
        }
        // The same AdaptationSet@id in different Periods is fine.
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period duration="PT2S"><AdaptationSet id="1"/></Period><Period><AdaptationSet id="1"/></Period></MPD>"#;
        assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
    }

    #[test]
    fn addressing_conflict_at_one_level_is_malformed() {
        for inner in [
            r#"<SegmentBase/><SegmentTemplate duration="2" media="$Number$"/>"#,
            r#"<SegmentList duration="2"/><SegmentBase/>"#,
            r#"<SegmentTemplate duration="2" media="$Number$"/><SegmentList duration="2"/>"#,
        ] {
            let xml = wrap_set(&format!(
                r#"<Representation id="v" bandwidth="1">{inner}</Representation>"#
            ));
            assert!(
                matches!(
                    Manifest::parse(xml.as_bytes(), &url()),
                    Err(ManifestError::Inconsistent { .. })
                ),
                "{inner}"
            );
        }
        // Different levels are inheritance, not a conflict.
        let xml = wrap_set(
            r#"<SegmentTemplate duration="2" media="$Number$"/><Representation id="v" bandwidth="1"><SegmentBase/></Representation>"#,
        );
        let manifest = Manifest::parse(xml.as_bytes(), &url()).unwrap();
        assert_eq!(
            manifest.periods[0].rejected[0].feature,
            UnsupportedFeature::UnindexedResource
        );
    }

    fn subsegments(subsegments: &[(u64, u64, u64)]) -> Vec<super::super::Subsegment> {
        subsegments
            .iter()
            .map(|&(first, last, duration)| super::super::Subsegment {
                range: ByteRange { first, last },
                duration,
            })
            .collect()
    }

    fn indexed(entries: &[(u64, u64, u64)]) -> SubsegmentIndex {
        SubsegmentIndex::new(1000, 500, 900, &subsegments(entries)).unwrap()
    }

    #[test]
    fn segment_base_waits_for_its_index() {
        let mut parsed = manifest(&wrap_set(
            r#"<Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="900-1099"/></Representation>"#,
        ));
        let representation = &mut parsed.periods[0].adaptation_sets[0].representations[0];
        assert_eq!(representation.addressing_kind(), AddressingKind::Indexed);
        let resource = Url::parse("https://origin.example.com/live/v.mp4").unwrap();
        assert_eq!(
            representation.pending_segment_index(),
            Some(&SegmentIndexLocation {
                url: resource.clone(),
                range: ByteRange {
                    first: 900,
                    last: 1099
                },
            })
        );
        // The initialization data precedes the index box, which may start
        // after `@indexRange` does; it is known once the index is read.
        assert_eq!(representation.initialization, None);
        assert!(matches!(
            representation.segments(at(0)),
            Err(ManifestError::SegmentIndexPending { .. })
        ));

        representation
            .apply_segment_index(&indexed(&[
                (1100, 1999, 2000),
                (2000, 2999, 2000),
                (3000, 3499, 1000),
            ]))
            .unwrap();
        assert_eq!(representation.pending_segment_index(), None);
        assert_eq!(
            representation.initialization,
            Some(InitSegment {
                url: resource.clone(),
                byte_range: Some(ByteRange {
                    first: 0,
                    last: 899
                }),
            })
        );
        assert!(representation.uses_segment_timeline());
        let segments: Vec<_> = representation
            .segments(at(0))
            .unwrap()
            .iter()
            .map(|segment| {
                let segment = segment.unwrap();
                assert_eq!(segment.url, resource);
                (
                    segment.number,
                    segment.time,
                    segment.duration,
                    segment.byte_range.map(|range| (range.first, range.last)),
                )
            })
            .collect();
        assert_eq!(
            segments,
            [
                (1, 500, 2000, Some((1100, 1999))),
                (2, 2500, 2000, Some((2000, 2999))),
                (3, 4500, 1000, Some((3000, 3499))),
            ]
        );
        assert!(
            representation
                .apply_segment_index(&indexed(&[(1, 2, 3)]))
                .is_err()
        );
    }

    #[test]
    fn segment_base_inherits_and_takes_explicit_initialization() {
        let mut parsed = manifest(&wrap_set(
            r#"<SegmentBase timescale="90000" presentationTimeOffset="45000" indexRange="1-2"><Initialization sourceURL="init.mp4"/></SegmentBase>
<Representation id="v" bandwidth="1"><SegmentBase indexRange="600-700"><Initialization range="0-599"/></SegmentBase></Representation>
<Representation id="w" bandwidth="2"/>"#,
        ));
        let set = &mut parsed.periods[0].adaptation_sets[0];
        let document = url();
        let v = &set.representations[0];
        assert_eq!(v.pending_segment_index().unwrap().range.first, 600);
        // The Representation's `Initialization` replaces the inherited one.
        assert_eq!(
            v.initialization,
            Some(InitSegment {
                url: document.clone(),
                byte_range: Some(ByteRange {
                    first: 0,
                    last: 599
                }),
            })
        );
        assert_eq!(v.timescale(), 90_000);
        let w = &mut set.representations[1];
        assert_eq!(
            w.initialization.as_ref().map(|init| init.url.as_str()),
            Some("https://origin.example.com/live/init.mp4")
        );

        // 45000 / 90000 s is 500 ticks of the index's 1000 Hz.
        w.apply_segment_index(&indexed(&[(3, 9, 1000)])).unwrap();
        assert_eq!((w.timescale(), w.presentation_time_offset()), (1000, 500));
        let first = w.segments(at(0)).unwrap().first().unwrap().unwrap();
        assert_eq!((first.time, first.start_secs), (500, 0.0));
    }

    #[test]
    fn an_unusable_segment_index_is_refused() {
        let xml = wrap_set(
            r#"<Representation id="v" bandwidth="1"><SegmentBase timescale="3" presentationTimeOffset="1" indexRange="0-9"/></Representation>"#,
        );
        for entries in [
            &[][..],
            &[(10, 20, 0)],
            &[(20, 10, 5)],
            &[(10, 20, 5), (20, 30, 5)],
            &[(10, 20, 5), (5, 8, 5)],
        ] {
            assert!(
                matches!(
                    SubsegmentIndex::new(1000, 0, 0, &subsegments(entries)),
                    Err(ManifestError::InvalidSegmentIndex { .. })
                ),
                "{entries:?}"
            );
        }
        // 1/3 s is not a whole number of milliseconds.
        let mut parsed = manifest(&xml);
        let representation = &mut parsed.periods[0].adaptation_sets[0].representations[0];
        assert!(matches!(
            representation.apply_segment_index(&indexed(&[(10, 20, 5)])),
            Err(ManifestError::Unsupported(
                UnsupportedFeature::FractionalPresentationTimeOffset
            ))
        ));
        assert!(representation.pending_segment_index().is_some());
    }

    #[test]
    fn segment_base_is_refused_in_dynamic_mpds() {
        let manifest = manifest(
            r#"<MPD type="dynamic" availabilityStartTime="2020-01-01T00:00:00Z"><Period start="PT0S">
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><SegmentBase indexRange="10-20"/></Representation></AdaptationSet>
</Period></MPD>"#,
        );
        assert_eq!(
            manifest.periods[0].rejected[0].feature,
            UnsupportedFeature::LiveSegmentBase
        );
    }

    #[test]
    fn segment_list_cardinality_matches_its_timeline() {
        let list = |timeline: &str, urls: usize| {
            let urls: String = (0..urls)
                .map(|i| format!(r#"<SegmentURL media="{i}.m4s"/>"#))
                .collect();
            wrap_set(&format!(
                r#"<SegmentList><SegmentTimeline>{timeline}</SegmentTimeline>{urls}</SegmentList><Representation id="v" bandwidth="1"/>"#
            ))
        };
        let parse = |xml: String| Manifest::parse(xml.as_bytes(), &url());
        // Two S entries with r=1 describe four segments.
        assert!(parse(list(r#"<S t="0" d="2" r="1"/><S d="2" r="1"/>"#, 4)).is_ok());
        for urls in [3, 5] {
            assert!(
                matches!(
                    parse(list(r#"<S t="0" d="2" r="1"/><S d="2" r="1"/>"#, urls)),
                    Err(ManifestError::Inconsistent { .. })
                ),
                "{urls}"
            );
        }
        // A trailing r=-1 takes the remaining URLs, at least one.
        let manifest = parse(list(r#"<S t="0" d="1"/><S d="2" r="-1"/>"#, 3)).unwrap();
        let representation = &manifest.periods[0].adaptation_sets[0].representations[0];
        assert!(representation.uses_segment_timeline());
        let segments: Vec<_> = representation
            .segments(at(0))
            .unwrap()
            .iter()
            .map(|segment| {
                let segment = segment.unwrap();
                (segment.time, segment.duration)
            })
            .collect();
        assert_eq!(segments, [(0, 1), (1, 2), (3, 2)]);
        assert!(matches!(
            parse(list(r#"<S t="0" d="1"/><S d="2" r="-1"/>"#, 1)),
            Err(ManifestError::Inconsistent { .. })
        ));
    }

    #[test]
    fn mpd_chaining_and_unknown_essential_properties_are_unsupported() {
        for (xml, expected) in [
            (
                r#"<MPD type="static"><SupplementalProperty schemeIdUri="urn:mpeg:dash:mpd-chaining:2016" value="https://e.com/next.mpd"/><Period duration="PT1S"/></MPD>"#,
                UnsupportedFeature::MpdChaining("urn:mpeg:dash:mpd-chaining:2016".into()),
            ),
            (
                r#"<MPD type="static"><Period duration="PT1S"><EssentialProperty schemeIdUri="urn:mpeg:dash:fallback:2016" value="https://e.com/b.mpd"/></Period></MPD>"#,
                UnsupportedFeature::MpdChaining("urn:mpeg:dash:fallback:2016".into()),
            ),
            (
                r#"<MPD type="static"><EssentialProperty schemeIdUri="urn:example:must-understand"/><Period duration="PT1S"/></MPD>"#,
                UnsupportedFeature::EssentialProperty("urn:example:must-understand".into()),
            ),
        ] {
            let error = Manifest::parse(xml.as_bytes(), &url()).unwrap_err();
            assert!(
                matches!(&error, ManifestError::Unsupported(feature) if *feature == expected),
                "{xml}: {error}"
            );
        }
        // Unknown supplemental properties are optional and ignored.
        let xml = r#"<MPD type="static"><SupplementalProperty schemeIdUri="urn:example:hint"/><Period duration="PT1S"/></MPD>"#;
        assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
    }

    #[test]
    fn end_number_below_start_number_is_malformed() {
        let xml = wrap_set(
            r#"<SegmentTemplate duration="2" media="$Number$" startNumber="10" endNumber="9"/><Representation id="v" bandwidth="1"/>"#,
        );
        assert!(matches!(
            Manifest::parse(xml.as_bytes(), &url()),
            Err(ManifestError::Inconsistent { .. })
        ));
    }
}
