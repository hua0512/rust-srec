// HLS (HTTP Live Streaming) segment data handling
pub mod mp4;
pub mod profile;
pub mod resolution;
pub mod segment;
pub mod ts;

// Export common types for ease of use
pub use media_types::Resolution;
pub use mp4::{M4sData, M4sInitSegmentData, M4sSegmentData};
pub use profile::{SegmentType, StreamProfile, StreamProfileOptions};
pub use resolution::ResolutionDetector;
pub use segment::HlsData;
pub use ts::{ProgramInfo, StreamEntry, TsSegmentData, TsStreamInfo};
