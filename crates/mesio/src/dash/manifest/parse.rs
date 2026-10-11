//! MPD XML → raw element model (ISO/IEC 23009-1 §5.3, Annex B schema).
//!
//! The raw model keeps attributes as they appear at each level, so the
//! resolver can apply inheritance. Elements and attributes outside this
//! engine's needs are ignored, as are elements from other namespaces.
//! Element counts and string lengths are bounded by [`super::limits`].

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use roxmltree::{Document, Node, ParsingOptions};

use super::error::ManifestError;
use super::limits::{
    MAX_ATTRIBUTE_BYTES, MAX_MPD_BYTES, MAX_PERIODS, MAX_REPRESENTATIONS, MAX_SEGMENT_URLS,
    MAX_TIMELINE_ENTRIES, MAX_URL_BYTES,
};
use super::resolve::PresentationType;
use super::timeline::ByteRange;
use super::xs::{self, RawOffset};

const DASH_NS: &str = "urn:mpeg:dash:schema:mpd:2011";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

#[derive(Debug)]
pub(crate) struct RawMpd {
    pub presentation_type: PresentationType,
    pub availability_start_time: Option<DateTime<Utc>>,
    pub availability_end_time: Option<DateTime<Utc>>,
    pub publish_time: Option<DateTime<Utc>>,
    pub media_presentation_duration: Option<Duration>,
    pub minimum_update_period: Option<Duration>,
    pub min_buffer_time: Option<Duration>,
    pub time_shift_buffer_depth: Option<Duration>,
    pub suggested_presentation_delay: Option<Duration>,
    pub max_segment_duration: Option<Duration>,
    pub base_url: Option<RawBaseUrl>,
    pub location: Option<String>,
    pub utc_timings: Vec<RawUtcTiming>,
    pub properties: Vec<RawProperty>,
    pub periods: Vec<RawPeriod>,
}

#[derive(Debug, Clone)]
pub(crate) struct RawUtcTiming {
    pub scheme_id_uri: String,
    pub value: Option<String>,
}

/// An `EssentialProperty` or `SupplementalProperty` descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawProperty {
    pub essential: bool,
    pub scheme_id_uri: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RawBaseUrl {
    pub url: String,
    pub availability_time_offset: Option<RawOffset>,
    pub availability_time_complete: Option<bool>,
    pub has_byte_range: bool,
}

/// `SegmentBase` / `SegmentList` / `SegmentTemplate` present at one level.
#[derive(Debug, Default)]
pub(crate) struct RawSegmentInfo {
    pub template: Option<RawSegmentTemplate>,
    pub list: Option<RawSegmentList>,
    pub has_base: bool,
}

impl RawSegmentInfo {
    /// At most one addressing scheme per element (ISO/IEC 23009-1 §5.3.9.1);
    /// two at the same level leave the addressing undefined.
    fn validate(&self, element: &'static str) -> Result<(), ManifestError> {
        let schemes = usize::from(self.template.is_some())
            + usize::from(self.list.is_some())
            + usize::from(self.has_base);
        if schemes > 1 {
            return Err(ManifestError::inconsistent(format!(
                "`{element}` has more than one of SegmentBase, SegmentList and SegmentTemplate"
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct RawPeriod {
    pub id: Option<String>,
    pub start: Option<Duration>,
    pub duration: Option<Duration>,
    pub has_xlink: bool,
    pub base_url: Option<RawBaseUrl>,
    pub properties: Vec<RawProperty>,
    pub segment_info: RawSegmentInfo,
    pub adaptation_sets: Vec<RawAdaptationSet>,
}

/// Attributes and descriptors shared by `AdaptationSet` and `Representation`
/// (ISO/IEC 23009-1 §5.3.7, `RepresentationBaseType`).
#[derive(Debug, Default)]
pub(crate) struct RawCommon {
    pub mime_type: Option<String>,
    pub codecs: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub start_with_sap: Option<u8>,
    pub has_content_protection: bool,
    pub essential_properties: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct RawAdaptationSet {
    pub id: Option<String>,
    pub content_type: Option<String>,
    pub lang: Option<String>,
    pub segment_alignment: bool,
    pub has_xlink: bool,
    pub common: RawCommon,
    pub base_url: Option<RawBaseUrl>,
    pub segment_info: RawSegmentInfo,
    pub representations: Vec<RawRepresentation>,
}

#[derive(Debug)]
pub(crate) struct RawRepresentation {
    pub id: String,
    pub bandwidth: u64,
    pub common: RawCommon,
    pub base_url: Option<RawBaseUrl>,
    pub segment_info: RawSegmentInfo,
}

/// Attributes of `MultipleSegmentBaseType` (ISO/IEC 23009-1 §5.3.9.2), as
/// written at one level. Child lists are shared, not copied, when inherited.
#[derive(Debug, Default, Clone)]
pub(crate) struct RawMultipleSegmentBase {
    pub timescale: Option<u64>,
    pub presentation_time_offset: Option<u64>,
    pub duration: Option<u64>,
    pub start_number: Option<u64>,
    pub end_number: Option<u64>,
    pub ept_delta: Option<i64>,
    pub availability_time_offset: Option<RawOffset>,
    pub availability_time_complete: Option<bool>,
    pub timeline: Option<Arc<[RawS]>>,
    pub initialization: Option<RawUrl>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RawSegmentTemplate {
    pub base: RawMultipleSegmentBase,
    pub media: Option<String>,
    pub initialization: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RawSegmentList {
    pub base: RawMultipleSegmentBase,
    pub has_xlink: bool,
    pub segment_urls: Arc<[RawSegmentUrl]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawSegmentUrl {
    pub media: Option<String>,
    pub media_range: Option<ByteRange>,
}

/// An `Initialization` element (`URLType`).
#[derive(Debug, Clone)]
pub(crate) struct RawUrl {
    pub source_url: Option<String>,
    pub range: Option<ByteRange>,
}

/// A `SegmentTimeline/S` element (ISO/IEC 23009-1 §5.3.9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawS {
    pub t: Option<u64>,
    pub d: u64,
    /// Repeat count: `r + 1` occurrences; `-1` repeats until the next `S@t`
    /// or the end of the Period.
    pub r: i64,
    pub has_n: bool,
    pub k: Option<u64>,
}

/// Parses an MPD document.
pub(crate) fn parse_mpd(body: &[u8]) -> Result<RawMpd, ManifestError> {
    if body.len() > MAX_MPD_BYTES {
        return Err(ManifestError::TooLarge {
            size: body.len(),
            limit: MAX_MPD_BYTES,
        });
    }
    let text = std::str::from_utf8(body).map_err(|_| ManifestError::Encoding)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // DTDs stay disabled (the default): an MPD has none, and entity
    // expansion is an amplification vector.
    let document = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            ..ParsingOptions::default()
        },
    )?;
    let root = document.root_element();
    if !is_dash(root, "MPD") {
        return Err(ManifestError::NotAnMpd {
            found: root.tag_name().name().chars().take(64).collect(),
        });
    }
    let mpd = parse_mpd_element(root)?;
    let representations: usize = mpd
        .periods
        .iter()
        .flat_map(|period| &period.adaptation_sets)
        .map(|set| set.representations.len())
        .sum();
    if representations > MAX_REPRESENTATIONS {
        return Err(ManifestError::LimitExceeded {
            what: "Representations per MPD",
            limit: MAX_REPRESENTATIONS,
        });
    }
    Ok(mpd)
}

fn parse_mpd_element(node: Node<'_, '_>) -> Result<RawMpd, ManifestError> {
    const E: &str = "MPD";
    let presentation_type = match node.attribute("type").map(str::trim) {
        None | Some("static") => PresentationType::Static,
        Some("dynamic") => PresentationType::Dynamic,
        Some(other) => {
            return Err(ManifestError::invalid_attribute(
                E,
                "type",
                other,
                "expected static or dynamic",
            ));
        }
    };

    let mut mpd = RawMpd {
        presentation_type,
        availability_start_time: attr(node, E, "availabilityStartTime", xs::parse_date_time)?,
        availability_end_time: attr(node, E, "availabilityEndTime", xs::parse_date_time)?,
        publish_time: attr(node, E, "publishTime", xs::parse_date_time)?,
        media_presentation_duration: attr(
            node,
            E,
            "mediaPresentationDuration",
            xs::parse_duration,
        )?,
        minimum_update_period: attr(node, E, "minimumUpdatePeriod", xs::parse_duration)?,
        min_buffer_time: attr(node, E, "minBufferTime", xs::parse_duration)?,
        time_shift_buffer_depth: attr(node, E, "timeShiftBufferDepth", xs::parse_duration)?,
        suggested_presentation_delay: attr(
            node,
            E,
            "suggestedPresentationDelay",
            xs::parse_duration,
        )?,
        max_segment_duration: attr(node, E, "maxSegmentDuration", xs::parse_duration)?,
        base_url: None,
        location: None,
        utc_timings: Vec::new(),
        properties: Vec::new(),
        periods: Vec::new(),
    };

    for child in dash_children(node) {
        match child.tag_name().name() {
            "BaseURL" => first(&mut mpd.base_url, child, parse_base_url)?,
            "Location" => {
                if mpd.location.is_none() {
                    mpd.location = Some(element_text(child, "Location", MAX_URL_BYTES)?);
                }
            }
            "UTCTiming" => mpd.utc_timings.push(RawUtcTiming {
                scheme_id_uri: required_text(
                    child,
                    "UTCTiming",
                    "schemeIdUri",
                    MAX_ATTRIBUTE_BYTES,
                )?,
                value: text_attr(child, "value", MAX_URL_BYTES)?,
            }),
            "Period" => {
                if mpd.periods.len() == MAX_PERIODS {
                    return Err(ManifestError::LimitExceeded {
                        what: "Periods per MPD",
                        limit: MAX_PERIODS,
                    });
                }
                mpd.periods.push(parse_period(child)?);
            }
            name => parse_property(&mut mpd.properties, child, name)?,
        }
    }
    Ok(mpd)
}

fn parse_period(node: Node<'_, '_>) -> Result<RawPeriod, ManifestError> {
    const E: &str = "Period";
    let mut period = RawPeriod {
        id: text_attr(node, "id", MAX_ATTRIBUTE_BYTES)?,
        start: attr(node, E, "start", xs::parse_duration)?,
        duration: attr(node, E, "duration", xs::parse_duration)?,
        has_xlink: has_xlink(node),
        base_url: None,
        properties: Vec::new(),
        segment_info: RawSegmentInfo::default(),
        adaptation_sets: Vec::new(),
    };
    for child in dash_children(node) {
        match child.tag_name().name() {
            "BaseURL" => first(&mut period.base_url, child, parse_base_url)?,
            "AdaptationSet" => period.adaptation_sets.push(parse_adaptation_set(child)?),
            name => {
                parse_property(&mut period.properties, child, name)?;
                parse_segment_info(&mut period.segment_info, child)?;
            }
        }
    }
    period.segment_info.validate(E)?;
    Ok(period)
}

fn parse_adaptation_set(node: Node<'_, '_>) -> Result<RawAdaptationSet, ManifestError> {
    const E: &str = "AdaptationSet";
    let mut set = RawAdaptationSet {
        id: text_attr(node, "id", MAX_ATTRIBUTE_BYTES)?,
        content_type: text_attr(node, "contentType", MAX_ATTRIBUTE_BYTES)?,
        lang: text_attr(node, "lang", MAX_ATTRIBUTE_BYTES)?,
        segment_alignment: attr(node, E, "segmentAlignment", parse_conditional_uint)?
            .unwrap_or(false),
        has_xlink: has_xlink(node),
        common: parse_common(node, E)?,
        base_url: None,
        segment_info: RawSegmentInfo::default(),
        representations: Vec::new(),
    };
    for child in dash_children(node) {
        match child.tag_name().name() {
            "BaseURL" => first(&mut set.base_url, child, parse_base_url)?,
            "Representation" => set.representations.push(parse_representation(child)?),
            name => {
                parse_descriptor(&mut set.common, child, name)?;
                parse_segment_info(&mut set.segment_info, child)?;
            }
        }
    }
    set.segment_info.validate(E)?;
    Ok(set)
}

fn parse_representation(node: Node<'_, '_>) -> Result<RawRepresentation, ManifestError> {
    const E: &str = "Representation";
    let mut representation = RawRepresentation {
        id: required_text(node, E, "id", MAX_ATTRIBUTE_BYTES)?,
        bandwidth: required(node, E, "bandwidth", xs::parse_u64)?,
        common: parse_common(node, E)?,
        base_url: None,
        segment_info: RawSegmentInfo::default(),
    };
    for child in dash_children(node) {
        match child.tag_name().name() {
            "BaseURL" => first(&mut representation.base_url, child, parse_base_url)?,
            name => {
                parse_descriptor(&mut representation.common, child, name)?;
                parse_segment_info(&mut representation.segment_info, child)?;
            }
        }
    }
    representation.segment_info.validate(E)?;
    Ok(representation)
}

fn parse_common(node: Node<'_, '_>, element: &'static str) -> Result<RawCommon, ManifestError> {
    let start_with_sap = attr(node, element, "startWithSAP", |value| {
        xs::parse_u64(value)
            .ok()
            .filter(|&sap| sap <= 6)
            .map(|sap| sap as u8)
            .ok_or("expected a SAP type from 0 to 6")
    })?;
    Ok(RawCommon {
        mime_type: text_attr(node, "mimeType", MAX_ATTRIBUTE_BYTES)?,
        codecs: text_attr(node, "codecs", MAX_ATTRIBUTE_BYTES)?,
        width: attr(node, element, "width", parse_u32)?,
        height: attr(node, element, "height", parse_u32)?,
        start_with_sap,
        has_content_protection: false,
        essential_properties: Vec::new(),
    })
}

fn parse_descriptor(
    common: &mut RawCommon,
    node: Node<'_, '_>,
    name: &str,
) -> Result<(), ManifestError> {
    match name {
        "ContentProtection" => common.has_content_protection = true,
        "EssentialProperty" => common.essential_properties.push(required_text(
            node,
            "EssentialProperty",
            "schemeIdUri",
            MAX_ATTRIBUTE_BYTES,
        )?),
        _ => {}
    }
    Ok(())
}

/// Collects MPD- and Period-level `EssentialProperty` / `SupplementalProperty`.
fn parse_property(
    properties: &mut Vec<RawProperty>,
    node: Node<'_, '_>,
    name: &str,
) -> Result<(), ManifestError> {
    let (essential, element) = match name {
        "EssentialProperty" => (true, "EssentialProperty"),
        "SupplementalProperty" => (false, "SupplementalProperty"),
        _ => return Ok(()),
    };
    properties.push(RawProperty {
        essential,
        scheme_id_uri: required_text(node, element, "schemeIdUri", MAX_ATTRIBUTE_BYTES)?,
    });
    Ok(())
}

fn parse_segment_info(info: &mut RawSegmentInfo, node: Node<'_, '_>) -> Result<(), ManifestError> {
    match node.tag_name().name() {
        "SegmentTemplate" => first_only(
            &mut info.template,
            node,
            "SegmentTemplate",
            parse_segment_template,
        ),
        "SegmentList" => first_only(&mut info.list, node, "SegmentList", parse_segment_list),
        "SegmentBase" => {
            if std::mem::replace(&mut info.has_base, true) {
                return Err(ManifestError::DuplicateElement {
                    element: "SegmentBase",
                });
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn parse_multiple_segment_base(
    node: Node<'_, '_>,
    element: &'static str,
) -> Result<RawMultipleSegmentBase, ManifestError> {
    let mut base = RawMultipleSegmentBase {
        timescale: attr(node, element, "timescale", |value| {
            xs::parse_u64(value)
                .ok()
                .filter(|&timescale| timescale > 0)
                .ok_or("expected a positive integer")
        })?,
        presentation_time_offset: attr(node, element, "presentationTimeOffset", xs::parse_u64)?,
        duration: attr(node, element, "duration", |value| {
            xs::parse_u64(value)
                .ok()
                .filter(|&duration| duration > 0)
                .ok_or("expected a positive integer")
        })?,
        start_number: attr(node, element, "startNumber", xs::parse_u64)?,
        end_number: attr(node, element, "endNumber", xs::parse_u64)?,
        ept_delta: attr(node, element, "eptDelta", xs::parse_i64)?,
        availability_time_offset: attr(
            node,
            element,
            "availabilityTimeOffset",
            xs::parse_availability_time_offset,
        )?,
        availability_time_complete: attr(
            node,
            element,
            "availabilityTimeComplete",
            xs::parse_bool,
        )?,
        timeline: None,
        initialization: None,
    };
    for child in dash_children(node) {
        match child.tag_name().name() {
            "SegmentTimeline" => first_only(
                &mut base.timeline,
                child,
                "SegmentTimeline",
                parse_segment_timeline,
            )?,
            "Initialization" => {
                first_only(&mut base.initialization, child, "Initialization", parse_url)?
            }
            _ => {}
        }
    }
    Ok(base)
}

fn parse_segment_template(node: Node<'_, '_>) -> Result<RawSegmentTemplate, ManifestError> {
    Ok(RawSegmentTemplate {
        base: parse_multiple_segment_base(node, "SegmentTemplate")?,
        media: text_attr(node, "media", MAX_URL_BYTES)?,
        initialization: text_attr(node, "initialization", MAX_URL_BYTES)?,
    })
}

fn parse_segment_list(node: Node<'_, '_>) -> Result<RawSegmentList, ManifestError> {
    let base = parse_multiple_segment_base(node, "SegmentList")?;
    let mut segment_urls = Vec::new();
    for child in dash_children(node).filter(|child| child.tag_name().name() == "SegmentURL") {
        if segment_urls.len() == MAX_SEGMENT_URLS {
            return Err(ManifestError::LimitExceeded {
                what: "SegmentURL elements per SegmentList",
                limit: MAX_SEGMENT_URLS,
            });
        }
        segment_urls.push(RawSegmentUrl {
            media: text_attr(child, "media", MAX_URL_BYTES)?,
            media_range: attr(child, "SegmentURL", "mediaRange", parse_byte_range)?,
        });
    }
    Ok(RawSegmentList {
        base,
        has_xlink: has_xlink(node),
        segment_urls: segment_urls.into(),
    })
}

fn parse_segment_timeline(node: Node<'_, '_>) -> Result<Arc<[RawS]>, ManifestError> {
    const E: &str = "S";
    let mut entries = Vec::new();
    for s in dash_children(node).filter(|child| child.tag_name().name() == E) {
        if entries.len() == MAX_TIMELINE_ENTRIES {
            return Err(ManifestError::LimitExceeded {
                what: "S elements per SegmentTimeline",
                limit: MAX_TIMELINE_ENTRIES,
            });
        }
        let d = required(s, E, "d", xs::parse_u64)?;
        if d == 0 {
            return Err(ManifestError::invalid_attribute(
                E,
                "d",
                "0",
                "must be positive",
            ));
        }
        let r = attr(s, E, "r", xs::parse_i64)?.unwrap_or(0);
        if r < -1 {
            return Err(ManifestError::invalid_attribute(
                E,
                "r",
                &r.to_string(),
                "only -1 may be negative",
            ));
        }
        entries.push(RawS {
            t: attr(s, E, "t", xs::parse_u64)?,
            d,
            r,
            has_n: s.has_attribute("n"),
            k: attr(s, E, "k", xs::parse_u64)?,
        });
    }
    Ok(entries.into())
}

fn parse_url(node: Node<'_, '_>) -> Result<RawUrl, ManifestError> {
    Ok(RawUrl {
        source_url: text_attr(node, "sourceURL", MAX_URL_BYTES)?,
        range: attr(node, "Initialization", "range", parse_byte_range)?,
    })
}

fn parse_base_url(node: Node<'_, '_>) -> Result<RawBaseUrl, ManifestError> {
    const E: &str = "BaseURL";
    Ok(RawBaseUrl {
        url: element_text(node, E, MAX_URL_BYTES)?,
        availability_time_offset: attr(
            node,
            E,
            "availabilityTimeOffset",
            xs::parse_availability_time_offset,
        )?,
        availability_time_complete: attr(node, E, "availabilityTimeComplete", xs::parse_bool)?,
        has_byte_range: node.has_attribute("byteRange"),
    })
}

/// `ConditionalUintType`: `xs:unsignedInt` or `xs:boolean`. An integer means
/// "aligned with sets carrying the same value", which is aligned too.
fn parse_conditional_uint(value: &str) -> Result<bool, &'static str> {
    match value.trim() {
        "false" => Ok(false),
        "true" => Ok(true),
        other => xs::parse_u64(other)
            .map(|_| true)
            .map_err(|_| "expected true, false or an unsigned integer"),
    }
}

fn parse_u32(value: &str) -> Result<u32, &'static str> {
    xs::parse_u64(value)?
        .try_into()
        .map_err(|_| "value is too large")
}

/// Parses a single `first-last` byte range (RFC 9110 §14.1.1 without the
/// `bytes=` prefix), as `@mediaRange` and `@range` use it.
fn parse_byte_range(value: &str) -> Result<ByteRange, &'static str> {
    const INVALID: &str = "expected a byte range `first-last` with first <= last";
    let (first, last) = value.trim().split_once('-').ok_or(INVALID)?;
    let first = xs::parse_u64(first).map_err(|_| INVALID)?;
    let last = xs::parse_u64(last).map_err(|_| INVALID)?;
    if first > last {
        return Err(INVALID);
    }
    Ok(ByteRange { first, last })
}

fn is_dash(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace().is_none_or(|ns| ns == DASH_NS)
}

fn dash_children<'a, 'input>(
    node: Node<'a, 'input>,
) -> impl Iterator<Item = Node<'a, 'input>> + use<'a, 'input> {
    node.children().filter(|child| {
        child.is_element() && child.tag_name().namespace().is_none_or(|ns| ns == DASH_NS)
    })
}

fn has_xlink(node: Node<'_, '_>) -> bool {
    node.has_attribute((XLINK_NS, "href"))
}

fn attr<T>(
    node: Node<'_, '_>,
    element: &'static str,
    attribute: &'static str,
    parse: impl FnOnce(&str) -> Result<T, &'static str>,
) -> Result<Option<T>, ManifestError> {
    node.attribute(attribute)
        .map(|value| {
            parse(value).map_err(|reason| {
                ManifestError::invalid_attribute(element, attribute, value, reason)
            })
        })
        .transpose()
}

fn required<T>(
    node: Node<'_, '_>,
    element: &'static str,
    attribute: &'static str,
    parse: impl FnOnce(&str) -> Result<T, &'static str>,
) -> Result<T, ManifestError> {
    attr(node, element, attribute, parse)?
        .ok_or(ManifestError::MissingAttribute { element, attribute })
}

/// A trimmed string attribute of at most `limit` bytes.
fn text_attr(
    node: Node<'_, '_>,
    attribute: &'static str,
    limit: usize,
) -> Result<Option<String>, ManifestError> {
    node.attribute(attribute)
        .map(|value| bounded(value.trim(), attribute, limit))
        .transpose()
}

fn required_text(
    node: Node<'_, '_>,
    element: &'static str,
    attribute: &'static str,
    limit: usize,
) -> Result<String, ManifestError> {
    text_attr(node, attribute, limit)?.ok_or(ManifestError::MissingAttribute { element, attribute })
}

/// The trimmed text content of an element, of at most `limit` bytes.
fn element_text(
    node: Node<'_, '_>,
    element: &'static str,
    limit: usize,
) -> Result<String, ManifestError> {
    bounded(node.text().unwrap_or("").trim(), element, limit)
}

fn bounded(value: &str, what: &'static str, limit: usize) -> Result<String, ManifestError> {
    if value.len() > limit {
        return Err(ManifestError::LimitExceeded { what, limit });
    }
    Ok(value.to_owned())
}

/// Keeps the first occurrence of a repeatable element (v1 uses the first
/// `BaseURL` at each level).
fn first<T>(
    slot: &mut Option<T>,
    node: Node<'_, '_>,
    parse: impl FnOnce(Node<'_, '_>) -> Result<T, ManifestError>,
) -> Result<(), ManifestError> {
    if slot.is_none() {
        *slot = Some(parse(node)?);
    }
    Ok(())
}

/// Stores an element that the schema allows at most once per parent.
fn first_only<T>(
    slot: &mut Option<T>,
    node: Node<'_, '_>,
    element: &'static str,
    parse: impl FnOnce(Node<'_, '_>) -> Result<T, ManifestError>,
) -> Result<(), ManifestError> {
    if slot.is_some() {
        return Err(ManifestError::DuplicateElement { element });
    }
    *slot = Some(parse(node)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::timeline::AvailabilityTimeOffset;
    use super::*;

    fn parse(xml: &str) -> Result<RawMpd, ManifestError> {
        parse_mpd(xml.as_bytes())
    }

    #[test]
    fn parses_attributes_at_every_level() {
        let mpd = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" xmlns:cenc="urn:mpeg:cenc:2013"
     type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z"
     publishTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S"
     minBufferTime="PT4S" timeShiftBufferDepth="PT1M"
     suggestedPresentationDelay="PT6S" maxSegmentDuration="PT2S">
  <BaseURL availabilityTimeOffset="1.5">https://cdn.example.com/</BaseURL>
  <BaseURL>https://ignored.example.com/</BaseURL>
  <Location>https://example.com/live.mpd</Location>
  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:http-iso:2014" value="https://time.example.com/"/>
  <Period id="p0" start="PT0S">
    <AdaptationSet id="1" contentType="video" mimeType="video/mp4" segmentAlignment="true"
                   startWithSAP="1" lang="und">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <EssentialProperty schemeIdUri="http://dashif.org/guidelines/trickmode" value="1"/>
      <SegmentTemplate timescale="90000" media="$Number$.m4s" initialization="init.mp4"
                       duration="180000" startNumber="5" availabilityTimeOffset="INF"
                       availabilityTimeComplete="false"/>
      <Representation id="v1" bandwidth="3000000" width="1920" height="1080" codecs="avc1.640028">
        <SegmentTemplate presentationTimeOffset="900">
          <SegmentTimeline>
            <S t="10" d="20" r="-1"/>
            <S d="30" n="2" k="1"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
    <Unknown><Representation id="x" bandwidth="1"/></Unknown>
  </Period>
</MPD>"#,
        )
        .unwrap();

        assert_eq!(mpd.presentation_type, PresentationType::Dynamic);
        assert_eq!(mpd.minimum_update_period, Some(Duration::from_secs(2)));
        assert_eq!(mpd.time_shift_buffer_depth, Some(Duration::from_secs(60)));
        assert_eq!(
            mpd.suggested_presentation_delay,
            Some(Duration::from_secs(6))
        );
        let base = mpd.base_url.as_ref().unwrap();
        assert_eq!(base.url, "https://cdn.example.com/");
        assert_eq!(
            base.availability_time_offset,
            Some(RawOffset::Offset(AvailabilityTimeOffset::Finite(
                Duration::from_millis(1500)
            )))
        );
        assert_eq!(
            mpd.location.as_deref(),
            Some("https://example.com/live.mpd")
        );
        assert_eq!(mpd.utc_timings.len(), 1);

        let period = &mpd.periods[0];
        assert_eq!(period.id.as_deref(), Some("p0"));
        assert_eq!(period.adaptation_sets.len(), 1);
        let set = &period.adaptation_sets[0];
        assert_eq!(set.id.as_deref(), Some("1"));
        assert!(set.segment_alignment);
        assert!(set.common.has_content_protection);
        assert_eq!(set.common.start_with_sap, Some(1));
        assert_eq!(
            set.common.essential_properties,
            ["http://dashif.org/guidelines/trickmode"]
        );
        let template = set.segment_info.template.as_ref().unwrap();
        assert_eq!(template.base.timescale, Some(90_000));
        assert_eq!(template.base.start_number, Some(5));
        assert_eq!(
            template.base.availability_time_offset,
            Some(RawOffset::Offset(AvailabilityTimeOffset::Infinite))
        );
        assert_eq!(template.base.availability_time_complete, Some(false));

        let representation = &set.representations[0];
        assert_eq!(representation.bandwidth, 3_000_000);
        assert_eq!(representation.common.width, Some(1920));
        let timeline = representation.segment_info.template.as_ref().unwrap();
        let timeline = timeline.base.timeline.as_ref().unwrap();
        assert_eq!(
            timeline[0],
            RawS {
                t: Some(10),
                d: 20,
                r: -1,
                has_n: false,
                k: None
            }
        );
        assert!(timeline[1].has_n);
        assert_eq!(timeline[1].k, Some(1));
    }

    #[test]
    fn mpd_without_namespace_is_accepted() {
        let mpd = parse(r#"<MPD><Period/></MPD>"#).unwrap();
        assert_eq!(mpd.presentation_type, PresentationType::Static);
        assert_eq!(mpd.periods.len(), 1);
    }

    #[test]
    fn foreign_namespace_elements_are_ignored() {
        let mpd = parse(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" xmlns:x="urn:example">
                 <x:Period/><Period/></MPD>"#,
        )
        .unwrap();
        assert_eq!(mpd.periods.len(), 1);
    }

    #[test]
    fn rejects_malformed_documents() {
        let cases = [
            ("<MPD", "xml"),
            ("<NotMpd/>", "root"),
            (r#"<MPD type="live"/>"#, "type"),
            (r#"<MPD mediaPresentationDuration="10s"/>"#, "duration"),
            (r#"<MPD availabilityStartTime="yesterday"/>"#, "date"),
            (
                r#"<MPD><Period><AdaptationSet><Representation bandwidth="1"/></AdaptationSet></Period></MPD>"#,
                "missing id",
            ),
            (
                r#"<MPD><Period><AdaptationSet><Representation id="a"/></AdaptationSet></Period></MPD>"#,
                "missing bandwidth",
            ),
            (
                r#"<MPD><Period><SegmentTemplate timescale="0"/></Period></MPD>"#,
                "zero timescale",
            ),
            (
                r#"<MPD><Period><SegmentTemplate duration="0"/></Period></MPD>"#,
                "zero duration",
            ),
            (
                r#"<MPD><Period><SegmentTemplate/><SegmentTemplate/></Period></MPD>"#,
                "duplicate template",
            ),
            (
                r#"<MPD><Period><SegmentTemplate><SegmentTimeline><S t="0" d="0"/></SegmentTimeline></SegmentTemplate></Period></MPD>"#,
                "d=0",
            ),
            (
                r#"<MPD><Period><SegmentTemplate><SegmentTimeline><S d="1" r="-2"/></SegmentTimeline></SegmentTemplate></Period></MPD>"#,
                "r=-2",
            ),
            (
                r#"<MPD><Period><SegmentTemplate><SegmentTimeline><S r="1"/></SegmentTimeline></SegmentTemplate></Period></MPD>"#,
                "missing d",
            ),
            (
                r#"<MPD><Period><SegmentTemplate><SegmentTimeline><S d="99999999999999999999"/></SegmentTimeline></SegmentTemplate></Period></MPD>"#,
                "d overflow",
            ),
            (
                r#"<MPD><Period><SegmentList><SegmentURL media="a" mediaRange="10-5"/></SegmentList></Period></MPD>"#,
                "range",
            ),
            (
                r#"<MPD><Period><AdaptationSet startWithSAP="7"/></Period></MPD>"#,
                "sap",
            ),
            (
                r#"<MPD><BaseURL availabilityTimeOffset="NaN">a/</BaseURL></MPD>"#,
                "NaN ato",
            ),
            (r#"<!DOCTYPE MPD [<!ENTITY a "aaaa">]><MPD/>"#, "dtd"),
        ];
        for (xml, label) in cases {
            assert!(parse(xml).is_err(), "{label}");
        }
    }

    #[test]
    fn rejects_oversized_and_non_utf8_bodies() {
        let big = vec![b' '; MAX_MPD_BYTES + 1];
        assert!(matches!(
            parse_mpd(&big),
            Err(ManifestError::TooLarge { .. })
        ));
        assert!(matches!(
            parse_mpd(&[0xff, 0xfe, b'<']),
            Err(ManifestError::Encoding)
        ));
    }

    #[test]
    fn byte_order_mark_is_skipped() {
        assert!(parse("\u{feff}<MPD/>").is_ok());
    }
}
