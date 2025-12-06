//! Danmu (live comment) collection service module.
//!
//! This module provides functionality for collecting live comments (danmu/弹幕)
//! from streaming platforms during live sessions.

pub mod provider;
pub mod providers;
pub mod sampler;
pub mod service;
pub mod statistics;

pub use provider::{DanmuConnection, DanmuMessage, DanmuProvider, DanmuType};
pub use sampler::{DanmuSampler, FixedIntervalSampler, VelocitySampler};
pub use service::DanmuService;
pub use statistics::DanmuStatistics;
