//! Configuration service module.
//!
//! This module provides the Config Service with in-memory caching and
//! event broadcasting for configuration updates.

pub mod cache;
pub mod events;
pub mod service;

pub use cache::{CacheStats, ConfigCache};
pub use events::{ConfigEventBroadcaster, ConfigUpdateEvent, UpdateCoalescer};
pub use service::ConfigService;
