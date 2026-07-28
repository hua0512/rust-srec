//! Streamer management module.
//!
//! This module provides the StreamerManager which maintains in-memory
//! streamer metadata with write-through persistence to the database.

pub(crate) mod manager;
pub(crate) mod metadata;

pub use manager::StreamerManager;
pub use metadata::StreamerMetadata;
pub(crate) use metadata::{DEFAULT_OFFLINE_CHECK_COUNT, download_failure_threshold};
