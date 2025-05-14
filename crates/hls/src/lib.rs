// HLS (HTTP Live Streaming) parser implementation
pub mod segment;
pub mod segment_parser;

// Export common types for ease of use
pub use segment::{
    HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, SegmentType, TsSegmentData,
};
