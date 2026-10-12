//! Resource limits for MPD parsing and resolution.
//!
//! The body size cap bounds the XML tree, but resolution multiplies inherited
//! data by the number of Representations that inherit it (a long BaseURL or
//! `@codecs` on an AdaptationSet, a `$RepresentationID$` repeated in a
//! template). These limits keep the resolved model proportional to what a
//! real presentation needs; exceeding one is a [`ManifestError::LimitExceeded`]
//! raised before the corresponding allocation.
//!
//! [`ManifestError::LimitExceeded`]: super::ManifestError::LimitExceeded

/// Upper bound on the size of an MPD body.
///
/// Live MPDs with long `SegmentTimeline`s reach a few megabytes.
pub const MAX_MPD_BYTES: usize = 16 * 1024 * 1024;

/// Periods per MPD. Server-side ad insertion produces dozens.
pub const MAX_PERIODS: usize = 1_000;

/// Representations per MPD, across all Periods.
pub const MAX_REPRESENTATIONS: usize = 10_000;

/// `S` elements per `SegmentTimeline`. A day of variable-duration 2 s
/// segments without `@r` compression is 43 200.
pub const MAX_TIMELINE_ENTRIES: usize = 262_144;

/// `SegmentURL` elements per `SegmentList`.
pub const MAX_SEGMENT_URLS: usize = 262_144;

/// Length of any URL, URL template (after `$RepresentationID$` and
/// `$Bandwidth$` substitution, with the widest per-segment substitutions) or
/// resolved BaseURL. Common HTTP servers reject request lines above 8 KiB.
pub const MAX_URL_BYTES: usize = 32 * 1024;

/// Length of identifiers and descriptive attributes (`@id`, `@mimeType`,
/// `@codecs`, `@lang`, `@contentType`, `@schemeIdUri`).
pub const MAX_ATTRIBUTE_BYTES: usize = 4 * 1024;

/// Total bytes of per-Representation data created by resolution (identifiers,
/// inherited strings, resolved BaseURLs and compiled templates). A typical
/// MPD needs a few tens of kilobytes.
pub const MAX_RESOLVED_BYTES: usize = 64 * 1024 * 1024;
