//! Download engine abstraction.
//!
//! This module defines the `DownloadEngine` trait and related types
//! for abstracting different download backends (ffmpeg, streamlink, mesio).

mod traits;
mod ffmpeg;
mod streamlink;
mod mesio;

pub use traits::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadInfo, DownloadProgress,
    DownloadStatus, EngineType, SegmentEvent,
};
pub use ffmpeg::FfmpegEngine;
pub use streamlink::StreamlinkEngine;
pub use mesio::MesioEngine;
