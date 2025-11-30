use hls::HlsData;
use std::time::Duration;

/// Reason why a gap was skipped in the reorder buffer
#[derive(Debug, Clone)]
pub enum GapSkipReason {
    /// Gap skipped because count threshold was exceeded
    CountThreshold(u64),
    /// Gap skipped because duration threshold was exceeded
    DurationThreshold(Duration),
    /// Gap skipped because both count and duration thresholds were exceeded
    BothThresholds { count: u64, duration: Duration },
}

#[derive(Debug, Clone)]
pub enum HlsStreamEvent {
    Data(Box<HlsData>),
    PlaylistRefreshed {
        media_sequence_base: u64,
        target_duration: f64,
    },
    DiscontinuityTagEncountered {
        // Contextual info, e.g., sequence number before/after
        // For example, if associated with a specific m3u8_rs::MediaSegment.
        // media_segment_uri: String,
    },
    StreamEnded,
    /// A segment timed out and was skipped (Requirements 2.4)
    SegmentTimeout {
        /// The sequence number of the segment that timed out
        sequence_number: u64,
        /// How long we waited before timing out
        waited_duration: Duration,
    },
    /// Gap was skipped due to strategy threshold (Requirements 6.2)
    GapSkipped {
        /// The sequence number we were waiting for
        from_sequence: u64,
        /// The sequence number we skipped to
        to_sequence: u64,
        /// The reason the gap was skipped
        reason: GapSkipReason,
    },
}
