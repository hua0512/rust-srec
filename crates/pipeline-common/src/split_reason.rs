//! Re-exports the split-reason types, which now live in the `media-types`
//! crate, so pipeline consumers can keep importing them via `pipeline_common`.
pub use media_types::split_reason::{AudioCodecInfo, SplitReason, VideoCodecInfo};
