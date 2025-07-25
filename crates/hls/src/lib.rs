// HLS (HTTP Live Streaming) parser implementation
pub mod resolution;
pub mod segment;
pub mod segment_parser;

// Export common types for ease of use
pub use resolution::{Resolution, ResolutionDetector};
pub use segment::{
    HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, ProgramInfo, SegmentType, StreamEntry,
    StreamProfile, TsSegmentData, TsStreamInfo,
};
