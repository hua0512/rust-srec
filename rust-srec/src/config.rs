//! Configuration service module.
//!
//! This module provides the Config Service with in-memory caching and
//! event broadcasting for configuration updates.

pub(crate) mod backup;
pub(crate) mod cache;
pub(crate) mod events;
mod merged;
mod resolver;
pub(crate) mod service;

pub use cache::ConfigCache;
pub use events::{ConfigEventBroadcaster, ConfigUpdateEvent};
pub use merged::{
    GlobalConfigLayer, MergedConfig, MergedConfigBuilder, PlatformConfigLayer, TemplateConfigLayer,
};
pub use resolver::ConfigResolver;
pub(crate) use resolver::parse_extractor;
pub use service::ConfigService;

/// The keys a streamer-specific config or a template platform override
/// keeps its pipelines under: each segment, after each session, and after
/// each paired segment.
pub(crate) const PIPELINE_FIELD: &str = "pipeline";
pub(crate) const SESSION_COMPLETE_PIPELINE_FIELD: &str = "session_complete_pipeline";
pub(crate) const PAIRED_SEGMENT_PIPELINE_FIELD: &str = "paired_segment_pipeline";
pub(crate) const PIPELINE_FIELDS: [&str; 3] = [
    PIPELINE_FIELD,
    SESSION_COMPLETE_PIPELINE_FIELD,
    PAIRED_SEGMENT_PIPELINE_FIELD,
];
