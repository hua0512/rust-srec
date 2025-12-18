//! Danmu (live comment) collection service module.
//!
//! This module provides functionality for collecting live comments (danmu/弹幕)
//! from streaming platforms during live sessions.
//!
//! Core types are re-exported from the `danmaku` crate for reusability.

// Re-export core types from danmaku crate
pub use danmaku::{
    DanmuConnection, DanmuMessage, DanmuProvider, DanmuSampler, DanmuSamplingConfig,
    DanmuStatistics, DanmuType, FixedIntervalSampler, HuyaDanmuProvider, ProviderRegistry,
    RateDataPoint, StatisticsAggregator, TopTalker, TwitchDanmuProvider, VelocitySampler,
    WordFrequency, XmlDanmuWriter, create_sampler, escape_xml, message_type_to_int,
};

// Local modules (application-specific)
pub mod events;
pub mod service;

pub use events::DanmuEvent;
pub use service::DanmuService;
