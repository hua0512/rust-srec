//! MPEG-DASH (ISO/IEC 23009-1) support.
//!
//! [`manifest`] parses and resolves Media Presentation Descriptions and
//! computes which segments are available at a given wall-clock time. It is
//! pure: it performs no network I/O.
//!
//! [`DashDownloader`] records a presentation: an MPD watcher publishes
//! snapshots, one segment pipeline per selected track (video, audio)
//! downloads that track's segments in order, and a muxer combines the tracks
//! into one fragmented MP4 stream of [`hls::HlsData`] items, so DASH output
//! is processed and written like HLS fMP4.

pub mod config;
mod dash_downloader;
mod engine;
pub mod manifest;
mod mux;
mod planner;
mod segment_index;
mod selection;
pub mod watcher;

pub use crate::segment_engine::descriptor::{DashSegmentMeta, MediaTrack};
pub use config::{
    AudioSelection, DashBudgetSplit, DashConfig, DashManifestConfig, DashMuxConfig,
    DashSelectionConfig, LiveStartPolicy, VideoSelection,
};
pub use dash_downloader::DashDownloader;
pub use watcher::MpdSnapshot;
