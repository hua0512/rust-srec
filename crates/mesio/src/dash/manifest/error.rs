use std::fmt;

use crate::error::{DownloadError, SharedError};

/// Why an MPD could not be parsed, resolved or evaluated.
///
/// Error messages never echo URLs or URL templates, which can carry signed
/// tokens.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("MPD is {size} bytes, above the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },

    #[error("MPD is not valid UTF-8")]
    Encoding,

    #[error("MPD is not well-formed XML: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("root element is `{found}`, expected `MPD`")]
    NotAnMpd { found: String },

    #[error("`{element}` is missing required attribute `{attribute}`")]
    MissingAttribute {
        element: &'static str,
        attribute: &'static str,
    },

    #[error("`{element}@{attribute}` has invalid value `{value}`: {reason}")]
    InvalidAttribute {
        element: &'static str,
        attribute: &'static str,
        /// The offending value, truncated; never set for URL-bearing attributes.
        value: String,
        reason: &'static str,
    },

    #[error("`{element}` appears more than once where at most one is allowed")]
    DuplicateElement { element: &'static str },

    #[error("invalid URL template in `{attribute}`: {reason}")]
    InvalidTemplate {
        attribute: &'static str,
        reason: String,
    },

    #[error("invalid URL in `{element}`: {source}")]
    InvalidUrl {
        element: &'static str,
        source: url::ParseError,
    },

    #[error("invalid SegmentTimeline: {reason}")]
    InvalidTimeline { reason: String },

    #[error("inconsistent MPD: {reason}")]
    Inconsistent { reason: String },

    /// A resource limit from [`super::limits`] was reached.
    #[error("{what} exceeds the limit of {limit}")]
    LimitExceeded { what: &'static str, limit: usize },

    #[error("arithmetic overflow while computing {context}")]
    Overflow { context: &'static str },

    #[error("invalid {context} time value")]
    InvalidTime { context: &'static str },

    /// A Segment Index (`sidx`) that cannot describe the Representation's
    /// subsegments: malformed, or inconsistent with the MPD.
    #[error("invalid Segment Index: {reason}")]
    InvalidSegmentIndex { reason: String },

    /// Segments of a `SegmentBase` Representation were requested before its
    /// Segment Index was read (see `Representation::apply_segment_index`).
    #[error("the Segment Index of Representation `{representation}` has not been read")]
    SegmentIndexPending { representation: String },

    /// Reading or applying a `SegmentBase` Representation's Segment Index
    /// failed (see `Representation::fail_segment_index`). Surfaces as
    /// `cause` itself.
    #[error("the Segment Index of Representation `{representation}` could not be read: {cause}")]
    SegmentIndexUnreadable {
        representation: String,
        cause: SharedError,
    },

    #[error("unsupported feature: {0}")]
    Unsupported(#[from] UnsupportedFeature),
}

impl ManifestError {
    pub(crate) fn invalid_attribute(
        element: &'static str,
        attribute: &'static str,
        value: &str,
        reason: &'static str,
    ) -> Self {
        const MAX_ECHO: usize = 64;
        let value = match value.char_indices().nth(MAX_ECHO) {
            Some((end, _)) => format!("{}...", &value[..end]),
            None => value.to_owned(),
        };
        Self::InvalidAttribute {
            element,
            attribute,
            value,
            reason,
        }
    }

    pub(crate) fn overflow(context: &'static str) -> Self {
        Self::Overflow { context }
    }

    pub(crate) fn timeline(reason: impl Into<String>) -> Self {
        Self::InvalidTimeline {
            reason: reason.into(),
        }
    }

    pub(crate) fn segment_index(reason: impl Into<String>) -> Self {
        Self::InvalidSegmentIndex {
            reason: reason.into(),
        }
    }

    pub(crate) fn inconsistent(reason: impl Into<String>) -> Self {
        Self::Inconsistent {
            reason: reason.into(),
        }
    }
}

/// A valid DASH feature outside this engine's scope.
///
/// The engine records unencrypted ISO BMFF content addressed through
/// `SegmentTemplate`, `SegmentList` or, in static MPDs, `SegmentBase`;
/// everything listed here is reported instead of being approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedFeature {
    /// `ContentProtection` (DRM or common encryption).
    ContentProtection,
    /// Audio, video or undeclared content in a container other than ISO
    /// BMFF audio or video (`video/mp4`, `audio/mp4`), e.g. WebM.
    MimeType(String),
    /// Content that is neither audio nor video (subtitles and captions,
    /// images, metadata), with its MIME type: only audio and video are
    /// recorded, whatever the container.
    NotAudioVideo(String),
    /// A Representation that is a single resource without a Segment Index:
    /// no segment information, or a `SegmentBase` without `@indexRange`.
    UnindexedResource,
    /// A `SegmentBase` whose Segment Index is a separate resource
    /// (`RepresentationIndex`).
    RepresentationIndex,
    /// `SegmentBase` in a dynamic MPD, where the indexed resource may still
    /// grow.
    LiveSegmentBase,
    /// A `SegmentBase@presentationTimeOffset` that is not a whole number of
    /// ticks of the Segment Index's timescale, so subsegment times cannot be
    /// placed on the Period timeline exactly.
    FractionalPresentationTimeOffset,
    /// Remote elements (`xlink:href`, ISO/IEC 23009-1 §5.5).
    XLink { element: &'static str },
    /// An `EssentialProperty` whose scheme must be understood to use the
    /// element (ISO/IEC 23009-1 §5.8.4, EssentialProperty).
    EssentialProperty(String),
    /// `BaseURL@byteRange` (ISO/IEC 23009-1 §5.6).
    BaseUrlByteRange,
    /// A non-zero `@eptDelta` (ISO/IEC 23009-1 5th ed.).
    EptDelta,
    /// `SegmentTemplate` without `@duration` or `SegmentTimeline`, or a
    /// `SegmentList` without either and more than one segment.
    UntimedSegments,
    /// `S@n` or `S@k` in a `SegmentTimeline`.
    TimelineAttribute(&'static str),
    /// A template identifier defined by the standard but not implemented,
    /// e.g. `$SubNumber$`.
    TemplateIdentifier(String),
    /// `availabilityTimeOffset="INF"` on a dynamic Period without an end and
    /// with unbounded addressing: every segment up to infinity would be
    /// available.
    UnboundedAvailability,
    /// A negative `@availabilityTimeOffset`: a valid `xs:double` that the
    /// standard gives no meaning.
    NegativeAvailabilityTimeOffset,
    /// MPD chaining or fallback (`urn:mpeg:dash:mpd-chaining:2016`,
    /// `urn:mpeg:dash:fallback:2016`), signalled by the given scheme.
    MpdChaining(String),
}

impl fmt::Display for UnsupportedFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentProtection => f.write_str("content protection (DRM)"),
            Self::MimeType(mime) => write!(
                f,
                "MIME type `{mime}` (only `video/mp4` and `audio/mp4` are supported)"
            ),
            Self::NotAudioVideo(mime) => write!(
                f,
                "`{mime}` text, image or metadata track (only audio and video are recorded)"
            ),
            Self::UnindexedResource => {
                f.write_str("single-resource Representation without a Segment Index")
            }
            Self::RepresentationIndex => f.write_str("SegmentBase with a RepresentationIndex"),
            Self::LiveSegmentBase => f.write_str("SegmentBase in a dynamic MPD"),
            Self::FractionalPresentationTimeOffset => f.write_str(
                "SegmentBase@presentationTimeOffset finer than the Segment Index timescale",
            ),
            Self::XLink { element } => write!(f, "xlink on `{element}`"),
            Self::EssentialProperty(scheme) => write!(f, "EssentialProperty `{scheme}`"),
            Self::BaseUrlByteRange => f.write_str("BaseURL@byteRange"),
            Self::EptDelta => f.write_str("non-zero @eptDelta"),
            Self::UntimedSegments => f.write_str("segment information without timing"),
            Self::TimelineAttribute(attr) => write!(f, "SegmentTimeline S@{attr}"),
            Self::TemplateIdentifier(id) => write!(f, "template identifier `${id}$`"),
            Self::UnboundedAvailability => {
                f.write_str("availabilityTimeOffset=INF on an open dynamic Period")
            }
            Self::NegativeAvailabilityTimeOffset => f.write_str("negative availabilityTimeOffset"),
            Self::MpdChaining(scheme) => write!(f, "MPD chaining `{scheme}`"),
        }
    }
}

impl std::error::Error for UnsupportedFeature {}

impl From<ManifestError> for DownloadError {
    fn from(error: ManifestError) -> Self {
        match error {
            ManifestError::Unsupported(feature) => DownloadError::Unsupported {
                protocol: "dash",
                feature: feature.to_string(),
            },
            // The resource itself is wrong; fetching it again cannot help.
            error @ ManifestError::InvalidSegmentIndex { .. } => DownloadError::InvalidContent {
                protocol: "dash",
                reason: error.to_string(),
            },
            ManifestError::SegmentIndexUnreadable { cause, .. } => cause.to_error(),
            other => DownloadError::Manifest {
                reason: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_maps_to_non_recoverable_download_error() {
        let error: DownloadError =
            ManifestError::from(UnsupportedFeature::ContentProtection).into();
        assert!(matches!(
            &error,
            DownloadError::Unsupported {
                protocol: "dash",
                ..
            }
        ));
        assert!(error.is_non_recoverable_source_error());
    }

    #[test]
    fn malformed_manifest_maps_to_retryable_manifest_error() {
        let error: DownloadError = ManifestError::inconsistent("periods overlap").into();
        assert!(matches!(&error, DownloadError::Manifest { reason } if reason.contains("overlap")));
        assert!(!error.is_non_recoverable_source_error());
    }

    #[test]
    fn long_attribute_values_are_truncated() {
        let value = "9".repeat(200);
        let error = ManifestError::invalid_attribute("S", "d", &value, "too large");
        let message = error.to_string();
        assert!(message.len() < 150, "{message}");
        assert!(message.contains("..."), "{message}");
    }
}
