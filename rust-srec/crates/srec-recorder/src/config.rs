//! Configuration service module.
//!
//! This module provides the Config Service with in-memory caching and
//! event broadcasting for configuration updates.

pub mod backup;
pub mod cache;
pub mod events;
mod merged;
mod resolver;
pub mod service;

pub use cache::ConfigCache;
pub use events::{ConfigEventBroadcaster, ConfigUpdateEvent};
pub use merged::{
    GlobalConfigLayer, MergedConfig, MergedConfigBuilder, PlatformConfigLayer, TemplateConfigLayer,
};
pub use resolver::ConfigResolver;
pub use resolver::parse_extractor;
pub use service::ConfigService;

pub(crate) use srec_core::domain::step_routes::{
    PAIRED_SEGMENT_PIPELINE_FIELD, PIPELINE_FIELD, SESSION_COMPLETE_PIPELINE_FIELD,
};
