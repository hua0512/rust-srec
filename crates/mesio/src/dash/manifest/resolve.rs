//! Raw MPD → resolved presentation: Period timing, attribute and segment
//! information inheritance, BaseURL resolution and v1 scope checks.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use url::Url;

use super::clock::UtcTiming;
use super::error::{ManifestError, UnsupportedFeature};
use super::parse::{
    self, RawAdaptationSet, RawBaseUrl, RawCommon, RawMpd, RawMultipleSegmentBase, RawPeriod,
    RawRepresentation, RawSegmentInfo, RawSegmentList, RawSegmentTemplate,
};
use super::template::{Template, TemplateKind};
use super::timeline::{
    Addressing, AvailabilityTimeOffset, InitSegment, PeriodTiming, Run, SegmentUrls, SegmentWindow,
};
use super::xs;

/// `EssentialProperty` schemes that do not affect fetching or storing
/// segments: colour signalling (ISO/IEC 23001-8 CICP) carried in the
/// bitstream anyway. Any other `EssentialProperty` makes its element unusable
/// (ISO/IEC 23009-1 §5.8.4, EssentialProperty).
const UNDERSTOOD_ESSENTIAL_PROPERTIES: &[&str] = &[
    "urn:mpeg:mpegB:cicp:ColourPrimaries",
    "urn:mpeg:mpegB:cicp:TransferCharacteristics",
    "urn:mpeg:mpegB:cicp:MatrixCoefficients",
];

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
}

/// A resolved Period.
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
    pub feature: UnsupportedFeature,
}

/// A resolved AdaptationSet.
#[derive(Debug, Clone)]
pub struct AdaptationSet {
    pub id: Option<String>,
    pub lang: Option<String>,
    pub segment_alignment: bool,
    pub representations: Vec<Representation>,
}

/// A resolved, supported Representation.
#[derive(Debug, Clone)]
pub struct Representation {
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
    timing: PeriodTiming,
}

impl Representation {
    pub fn timescale(&self) -> u64 {
        self.addressing.timescale
    }

    pub fn presentation_time_offset(&self) -> u64 {
        self.addressing.presentation_time_offset
    }

    /// The segments available at `now` (the server's time; see
    /// [`super::ClockOffset::apply`]). For static presentations, every segment
    /// of the Period.
    ///
    /// When `availability_time_complete` is false, a segment is only complete
    /// at its unadjusted availability time, so the availability time offset
    /// is not applied: this engine downloads whole segments.
    pub fn segments(&self, now: DateTime<Utc>) -> Result<SegmentWindow<'_>, ManifestError> {
        let offset = if self.availability_time_complete {
            self.availability_time_offset
        } else {
            AvailabilityTimeOffset::ZERO
        };
        let positions = self.addressing.window(&self.timing, offset, now)?;
        Ok(SegmentWindow::new(
            &self.addressing,
            self.timing.period_start,
            positions,
        ))
    }
}

impl Manifest {
    /// Parses and resolves an MPD fetched from `document_url` (the final URL
    /// after redirects, which relative BaseURLs resolve against).
    pub fn parse(body: &[u8], document_url: &Url) -> Result<Self, ManifestError> {
        resolve(parse::parse_mpd(body)?, document_url)
    }
}

fn resolve(raw: RawMpd, document_url: &Url) -> Result<Manifest, ManifestError> {
    let dynamic = raw.presentation_type == PresentationType::Dynamic;
    if dynamic && raw.availability_start_time.is_none() {
        return Err(ManifestError::MissingAttribute {
            element: "MPD",
            attribute: "availabilityStartTime",
        });
    }
    let mpd_base = BaseChain::new(document_url.clone()).join(raw.base_url.as_ref())?;

    // Period starts (ISO/IEC 23009-1 §5.3.2.1): explicit `@start`; else the
    // previous Period's start plus its `@duration`; else 0 for the first
    // Period of a static MPD. A dynamic Period whose start cannot be derived
    // is early available and not yet on the timeline.
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
        };
        periods.push(resolve_period(period, start, duration, timing, &mpd_base)?);
    }

    let utc_timings = raw
        .utc_timings
        .iter()
        .filter_map(|timing| {
            UtcTiming::from_descriptor(&timing.scheme_id_uri, timing.value.as_deref(), document_url)
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
    })
}

fn resolve_period(
    period: RawPeriod,
    start: Duration,
    duration: Option<Duration>,
    timing: PeriodTiming,
    mpd_base: &BaseChain,
) -> Result<Period, ManifestError> {
    // Representation@id is unique within a Period (ISO/IEC 23009-1 §5.3.5.2);
    // it identifies the selected Representation across MPD updates.
    let mut ids = HashSet::new();
    for representation in period
        .adaptation_sets
        .iter()
        .flat_map(|set| &set.representations)
    {
        if !ids.insert(representation.id.as_str()) {
            return Err(ManifestError::inconsistent(
                "duplicate Representation@id in a Period",
            ));
        }
    }

    let base = mpd_base.join(period.base_url.as_ref())?;
    let mut adaptation_sets = Vec::new();
    let mut rejected = Vec::new();
    for mut set in period.adaptation_sets {
        let representations = std::mem::take(&mut set.representations);
        let context = SetContext {
            set: &set,
            period_info: &period.segment_info,
            timing,
        };
        let set_base = match context.check_set(&base) {
            Ok(set_base) => set_base,
            Err(ManifestError::Unsupported(feature)) => {
                rejected.push(Rejected {
                    adaptation_set_id: set.id.clone(),
                    representation_id: None,
                    feature,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut supported = Vec::new();
        for representation in representations {
            let id = representation.id.clone();
            match context.resolve_representation(representation, &set_base) {
                Ok(representation) => supported.push(representation),
                Err(ManifestError::Unsupported(feature)) => rejected.push(Rejected {
                    adaptation_set_id: set.id.clone(),
                    representation_id: Some(id),
                    feature,
                }),
                Err(error) => return Err(error),
            }
        }
        if !supported.is_empty() {
            adaptation_sets.push(AdaptationSet {
                id: set.id,
                lang: set.lang.clone(),
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

struct SetContext<'a> {
    set: &'a RawAdaptationSet,
    period_info: &'a RawSegmentInfo,
    timing: PeriodTiming,
}

impl SetContext<'_> {
    fn check_set(&self, period_base: &BaseChain) -> Result<BaseChain, ManifestError> {
        if self.set.has_xlink {
            return Err(UnsupportedFeature::XLink {
                element: "AdaptationSet",
            }
            .into());
        }
        check_descriptors(&self.set.common)?;
        period_base.join(self.set.base_url.as_ref())
    }

    fn resolve_representation(
        &self,
        representation: RawRepresentation,
        set_base: &BaseChain,
    ) -> Result<Representation, ManifestError> {
        let set = self.set;
        check_descriptors(&representation.common)?;

        // Common attributes are inherited from the AdaptationSet
        // (ISO/IEC 23009-1 §5.3.7).
        let common = &representation.common;
        let mime_type = common
            .mime_type
            .clone()
            .or_else(|| set.common.mime_type.clone())
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
            _ => return Err(UnsupportedFeature::MimeType(mime_type).into()),
        };
        let content_type = match set.content_type.as_deref() {
            None => content_type,
            Some("video") => ContentType::Video,
            Some("audio") => ContentType::Audio,
            Some(other) => return Err(UnsupportedFeature::ContentType(other.to_owned()).into()),
        };

        let base = set_base.join(representation.base_url.as_ref())?;
        let info = merge_segment_info([
            self.period_info,
            &set.segment_info,
            &representation.segment_info,
        ])?;
        let built = build_addressing(
            &info,
            &representation.id,
            representation.bandwidth,
            &base.url,
        )?;
        let availability_time_offset = base
            .offset
            .checked_add(built.offset)
            .ok_or(ManifestError::overflow("availabilityTimeOffset"))?;

        Ok(Representation {
            id: representation.id,
            bandwidth: representation.bandwidth,
            content_type,
            mime_type,
            codecs: common.codecs.clone().or_else(|| set.common.codecs.clone()),
            width: common.width.or(set.common.width),
            height: common.height.or(set.common.height),
            start_with_sap: common.start_with_sap.or(set.common.start_with_sap),
            availability_time_offset,
            availability_time_complete: base.complete && built.complete,
            initialization: built.initialization,
            addressing: built.addressing,
            timing: self.timing,
        })
    }
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
        let url = self
            .url
            .join(&base_url.url)
            .map_err(|source| ManifestError::InvalidUrl {
                element: "BaseURL",
                source,
            })?;
        let offset = base_url
            .availability_time_offset
            .unwrap_or(AvailabilityTimeOffset::ZERO);
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
    Template(RawSegmentTemplate),
    List(RawSegmentList),
}

/// Merges `SegmentTemplate` or `SegmentList` from Period → AdaptationSet →
/// Representation: each attribute and child of a lower level overrides the
/// same one above it (ISO/IEC 23009-1 §5.3.9.1).
///
/// The innermost level carrying segment information decides the addressing
/// type. If that is a `SegmentBase` (or there is none at all, making the
/// Representation a single resource), the Representation uses indexed
/// addressing, which is out of scope. `SegmentTemplate` and `SegmentList` in
/// one chain are contradictory.
fn merge_segment_info(levels: [&RawSegmentInfo; 3]) -> Result<SegmentInfo, ManifestError> {
    let innermost = levels
        .iter()
        .rev()
        .find(|level| level.template.is_some() || level.list.is_some() || level.has_base);
    let Some(innermost) = innermost else {
        return Err(UnsupportedFeature::SegmentBase.into());
    };
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
        Ok(SegmentInfo::Template(merged))
    } else if innermost.list.is_some() {
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
                merged.segment_urls.clone_from(&list.segment_urls);
            }
        }
        Ok(SegmentInfo::List(merged))
    } else {
        Err(UnsupportedFeature::SegmentBase.into())
    }
}

impl RawMultipleSegmentBase {
    fn override_with(&mut self, inner: &Self) {
        fn take<T: Clone>(outer: &mut Option<T>, inner: &Option<T>) {
            if inner.is_some() {
                outer.clone_from(inner);
            }
        }
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

struct BuiltAddressing {
    addressing: Addressing,
    initialization: Option<InitSegment>,
    offset: AvailabilityTimeOffset,
    complete: bool,
}

fn build_addressing(
    info: &SegmentInfo,
    representation_id: &str,
    bandwidth: u64,
    base: &Url,
) -> Result<BuiltAddressing, ManifestError> {
    let (common, list_len) = match info {
        SegmentInfo::Template(template) => (&template.base, None),
        SegmentInfo::List(list) => (&list.base, Some(list.segment_urls.len())),
    };
    if common.ept_delta.is_some_and(|delta| delta != 0) {
        return Err(UnsupportedFeature::EptDelta.into());
    }
    let timescale = common.timescale.unwrap_or(1);
    let presentation_time_offset = common.presentation_time_offset.unwrap_or(0);
    let start_number = common.start_number.unwrap_or(1);

    let runs = match (common.duration, &common.timeline) {
        (Some(_), Some(_)) => {
            return Err(ManifestError::inconsistent(
                "both @duration and SegmentTimeline apply to a Representation",
            ));
        }
        // Simple addressing (DASH-IF timing model §18.4): segment k starts
        // k·@duration after the Period start, i.e. at media time
        // PTO + k·@duration.
        (Some(duration), None) => vec![Run {
            start: presentation_time_offset,
            duration,
            count: list_len.map(|len| len as u64),
            first_index: 0,
        }],
        (None, Some(entries)) => {
            if entries.iter().any(|entry| entry.has_n) {
                return Err(UnsupportedFeature::TimelineAttribute("n").into());
            }
            if entries.iter().any(|entry| entry.k.is_some_and(|k| k != 1)) {
                return Err(UnsupportedFeature::TimelineAttribute("k").into());
            }
            Addressing::timeline_runs(entries)?
        }
        (None, None) => return Err(UnsupportedFeature::UntimedSegments.into()),
    };

    let join = |reference: &str, element: &'static str| {
        base.join(reference)
            .map_err(|source| ManifestError::InvalidUrl { element, source })
    };
    let init_from_element = || -> Result<Option<InitSegment>, ManifestError> {
        common
            .initialization
            .as_ref()
            .map(|init| {
                // Without `@sourceURL` the BaseURL itself is the resource
                // (`URLType`, ISO/IEC 23009-1 §5.3.9.2).
                let url = match &init.source_url {
                    Some(source) => join(source, "Initialization@sourceURL")?,
                    None => base.clone(),
                };
                Ok(InitSegment {
                    url,
                    byte_range: init.range,
                })
            })
            .transpose()
    };

    let (urls, initialization) = match info {
        SegmentInfo::Template(template) => {
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
            join(
                &media.expand(start_number, presentation_time_offset),
                "SegmentTemplate@media",
            )?;
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
        SegmentInfo::List(list) => {
            let urls = list
                .segment_urls
                .iter()
                .map(|segment| {
                    let url = match &segment.media {
                        Some(media) => join(media, "SegmentURL@media")?,
                        None => base.clone(),
                    };
                    Ok(InitSegment {
                        url,
                        byte_range: segment.media_range,
                    })
                })
                .collect::<Result<Vec<_>, ManifestError>>()?;
            (SegmentUrls::List(urls), init_from_element()?)
        }
    };

    Ok(BuiltAddressing {
        addressing: Addressing {
            timescale,
            presentation_time_offset,
            start_number,
            end_number: common.end_number,
            runs,
            urls,
        },
        initialization,
        offset: common
            .availability_time_offset
            .unwrap_or(AvailabilityTimeOffset::ZERO),
        complete: common.availability_time_complete.unwrap_or(true),
    })
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
      <Representation id="indexed" bandwidth="1">
        <SegmentBase indexRange="0-100"/>
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
                (Some("3"), Some("indexed"), UnsupportedFeature::SegmentBase),
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
            ("", UnsupportedFeature::SegmentBase),
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
}
