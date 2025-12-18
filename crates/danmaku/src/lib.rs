//! Danmaku: Live chat/comment collection library for streaming platforms.
//!
//! This crate provides core types and traits for collecting live comments (danmu/弾幕)
//! from streaming platforms.
//!
//! ## Core Types
//!
//! - [`DanmuMessage`] - A single chat message with user info and metadata
//! - [`DanmuType`] - Message type classification (chat, gift, superchat, etc.)
//! - [`DanmuProvider`] - Trait for platform-specific implementations
//! - [`DanmuConnection`] - Connection state for an active danmu stream
//!
//! ## Sampling
//!
//! - [`DanmuSampler`] - Trait for message sampling strategies
//! - [`FixedIntervalSampler`] - Sample at fixed time intervals
//! - [`VelocitySampler`] - Dynamic sampling based on message velocity
//!
//! ## Statistics
//!
//! - [`DanmuStatistics`] - Aggregated statistics for a collection session
//! - [`StatisticsAggregator`] - Accumulates messages and computes statistics
//!
//! ## Providers
//!
//! - [`providers::HuyaDanmuProvider`] - Huya platform provider
//! - [`providers::TwitchDanmuProvider`] - Twitch platform provider
//! - [`providers::ProviderRegistry`] - Registry for managing providers
//!
//! ## Output
//!
//! - [`XmlDanmuWriter`] - Write danmu messages to XML files

pub mod error;
pub mod message;
pub mod provider;
pub mod providers;
pub mod sampler;
pub mod statistics;
pub mod writer;

pub use error::{DanmakuError, Result};
pub use message::{DanmuMessage, DanmuType};
pub use provider::{DanmuConnection, DanmuProvider};
pub use providers::{HuyaDanmuProvider, ProviderRegistry, TwitchDanmuProvider};
pub use sampler::{
    DanmuSampler, DanmuSamplingConfig, FixedIntervalSampler, VelocitySampler, create_sampler,
};
pub use statistics::{
    DanmuStatistics, RateDataPoint, StatisticsAggregator, TopTalker, WordFrequency,
};
pub use writer::{XmlDanmuWriter, escape_xml, message_type_to_int};
