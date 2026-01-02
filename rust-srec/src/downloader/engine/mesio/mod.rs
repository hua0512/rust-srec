//! Mesio native download engine module.
//!
//! This module contains the Mesio download engine and its supporting components:
//!
//! - `MesioEngine` - The main engine that coordinates protocol detection and delegation
//! - `HlsDownloader` - HLS-specific download orchestrator
//! - `FlvDownloader` - FLV-specific download orchestrator
//! - `config` - Configuration mapping utilities for mesio protocol configs
//!
//! # FLV Fix Configuration
//!
//! When FLV processing is enabled, Mesio can further tune the `flv-fix` pipeline
//! via `MesioEngineConfig.flv_fix` (e.g. split detection mode and duplicate-tag
//! filtering parameters). Defaults preserve the legacy behavior (split on raw
//! sequence-header CRC32 changes).
//!
//! # Architecture
//!
//! The `MesioEngine` acts as a thin coordinator that:
//! 1. Detects the protocol type from the URL (HLS or FLV)
//! 2. Delegates to the appropriate downloader (`HlsDownloader` or `FlvDownloader`)
//! 3. Propagates results back to the caller
//!
//! Progress tracking is handled by the writers internally via `WriterTask` and
//! `WriterState`, eliminating the need for duplicate tracking in the engine.

pub mod config;
mod engine;
mod flv_downloader;
mod hls_downloader;

pub use engine::MesioEngine;
pub use flv_downloader::FlvDownloader;
pub use hls_downloader::{DownloadStats, HlsDownloader};
