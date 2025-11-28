//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

pub mod config;
pub mod database;
pub mod domain;
pub mod downloader;
pub mod error;
pub mod monitor;
pub mod scheduler;
pub mod services;
pub mod streamer;

pub use error::{Error, Result};
