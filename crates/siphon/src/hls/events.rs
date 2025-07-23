use hls::HlsData;

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
}
