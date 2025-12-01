//! Download engine abstraction.
//!
//! This module defines the `DownloadEngine` trait and related types
//! for abstracting different download backends (ffmpeg, streamlink, mesio).

mod ffmpeg;
mod mesio;
mod streamlink;
mod traits;

pub use ffmpeg::FfmpegEngine;
pub use mesio::MesioEngine;
pub use streamlink::StreamlinkEngine;
pub use traits::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadInfo, DownloadProgress, DownloadStatus,
    EngineType, SegmentEvent,
};
