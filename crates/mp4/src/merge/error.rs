use crate::fourcc::FourCc;

/// Why an initialization segment or media segment could not be merged.
///
/// `Malformed` means the bytes violate ISO/IEC 14496-12. `Unsupported` means
/// the merger does not handle the layout: either it is valid but rewriting it
/// could change what a player presents (encryption, external data, explicit
/// base offsets), or it uses versions or flags the specification does not
/// define, so its fields cannot be located safely. Neither is retryable with
/// the same input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    #[error("malformed `{fourcc}` box: {reason}")]
    Malformed {
        fourcc: FourCc,
        reason: &'static str,
    },

    #[error("unsupported `{fourcc}` box: {reason}")]
    Unsupported {
        fourcc: FourCc,
        reason: &'static str,
    },

    #[error("protected content (`{fourcc}`) cannot be merged")]
    Protected { fourcc: FourCc },

    #[error("missing `{fourcc}` box in `{parent}`")]
    MissingBox { fourcc: FourCc, parent: FourCc },

    #[error(
        "track fragment refers to track {track_id}, which the source initialization segment does not declare"
    )]
    UnknownTrack { track_id: u32 },

    #[error("source index {index} is out of range for {sources} merged tracks")]
    UnknownSource { index: usize, sources: usize },

    #[error("at least one initialization segment is required")]
    NoSources,

    /// A source initialization segment declares more than one track; only
    /// single-track sources can be merged.
    #[error("initialization segment has {count} tracks; a merge source must have one")]
    MultipleTracks { count: usize },

    #[error("{0} overflows its field")]
    Overflow(&'static str),

    #[error("fragment sequence numbers are exhausted")]
    SequenceExhausted,
}

impl MergeError {
    pub(crate) fn malformed(fourcc: &[u8; 4], reason: &'static str) -> Self {
        Self::Malformed {
            fourcc: FourCc(*fourcc),
            reason,
        }
    }

    pub(crate) fn unsupported(fourcc: &[u8; 4], reason: &'static str) -> Self {
        Self::Unsupported {
            fourcc: FourCc(*fourcc),
            reason,
        }
    }

    pub(crate) fn missing(fourcc: &[u8; 4], parent: &[u8; 4]) -> Self {
        Self::MissingBox {
            fourcc: FourCc(*fourcc),
            parent: FourCc(*parent),
        }
    }
}
