//! Streamer management module.
//!
//! This module provides the StreamerManager which maintains in-memory
//! streamer metadata with write-through persistence to the database.

pub mod manager;
pub mod metadata;

pub use manager::StreamerManager;
pub use metadata::StreamerMetadata;
