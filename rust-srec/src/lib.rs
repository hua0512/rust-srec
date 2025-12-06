//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

pub mod api;
pub mod config;
pub mod danmu;
pub mod database;
pub mod domain;
pub mod downloader;
pub mod error;
pub mod metrics;
pub mod monitor;
pub mod notification;
pub mod pipeline;
pub mod scheduler;
pub mod services;
pub mod streamer;

pub use error::{Error, Result};
