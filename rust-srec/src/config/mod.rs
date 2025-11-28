//! Configuration service module.
//!
//! This module provides the Config Service with in-memory caching and
//! event broadcasting for configuration updates.

pub mod cache;
pub mod events;
pub mod service;

pub use cache::{ConfigCache, CacheStats};
pub use events::{ConfigUpdateEvent, ConfigEventBroadcaster, UpdateCoalescer};
pub use service::ConfigService;
