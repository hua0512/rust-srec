//! Danmu (live comment) collection service module.
//!
//! This module provides functionality for collecting live comments (danmu/弹幕)
//! from streaming platforms during live sessions.
//!
//! Core types are re-exported from the `danmaku` crate for reusability.

// Re-export core types from platforms-parser
pub use platforms_parser::danmaku::{
    AggregatorState, DanmuConnection, DanmuControlEvent, DanmuItem, DanmuMessage, DanmuProvider,
    DanmuStatistics, DanmuType, HuyaDanmuProvider, ProviderRegistry, RateDataPoint,
    StatisticsAggregator, StatisticsConfig, TopTalker, TwitchDanmuProvider, WordFrequency,
    XmlDanmuWriter, escape_xml, message_type_to_int,
};

// Local modules (application-specific)
mod checkpoint;
pub mod events;
mod lifecycle;
mod runner;
pub mod service;
mod statistics_session;
#[cfg(test)]
pub(crate) mod test_support;

pub use events::DanmuEvent;
pub use lifecycle::CollectionSpec;
pub use service::DanmuService;
