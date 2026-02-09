// HLS (HTTP Live Streaming) segment data handling
pub mod mp4;
pub mod profile;
pub mod resolution;
pub mod segment;
pub mod ts;

// Export common types for ease of use
pub use mp4::{M4sData, M4sInitSegmentData, M4sSegmentData};
pub use profile::{SegmentType, StreamProfile};
pub use resolution::{Resolution, ResolutionDetector};
pub use segment::HlsData;
pub use ts::{ProgramInfo, StreamEntry, TsSegmentData, TsStreamInfo};
