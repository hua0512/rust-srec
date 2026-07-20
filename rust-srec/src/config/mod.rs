//! Configuration service module.
//!
//! This module provides the Config Service with in-memory caching and
//! event broadcasting for configuration updates.

pub mod cache;
mod context;
pub mod events;
mod merged;
mod resolver;
pub mod service;

pub use cache::{CacheStats, ConfigCache};
pub use context::ResolvedStreamerContext;
pub use events::{ConfigEventBroadcaster, ConfigUpdateEvent, UpdateCoalescer};
pub use merged::{MergedConfig, MergedConfigBuilder};
pub use resolver::ConfigResolver;
pub use service::ConfigService;
