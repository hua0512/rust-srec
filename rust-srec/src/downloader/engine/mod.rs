//! Download engine abstraction.
//!
//! This module defines the `DownloadEngine` trait and related types
//! for abstracting different download backends (ffmpeg, streamlink, mesio).
//!
//! # Mesio Engine
//!
//! The `MesioEngine` uses the `mesio` crate's factory pattern for protocol
//! detection and stream downloading. It supports HLS and FLV formats with:
//!
//! - Protocol auto-detection via `MesioDownloaderFactory::detect_protocol()`
//! - Stream consumption through async `Stream<Item=Result<HlsData/FlvData>>`
//! - File writing via `HlsWriter` and `FlvWriter` (wrapping `WriterTask`)
//! - Optional pipeline processing through `HlsPipeline` and `FlvPipeline`
//! - Callback-based segment event notifications
//!
//! # Events
//!
//! All engines emit `SegmentEvent` messages through the `DownloadHandle`'s
//! event channel to report progress, segment completion, and errors.

mod ffmpeg;
mod mesio;
mod streamlink;
mod traits;
pub mod utils;

pub use ffmpeg::FfmpegEngine;
pub use mesio::{DownloadStats, FlvDownloader, HlsDownloader, MesioEngine, config};
pub use streamlink::StreamlinkEngine;
pub use traits::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadInfo,
    DownloadProgress, DownloadStatus, EngineType, SegmentEvent, SegmentInfo,
};
