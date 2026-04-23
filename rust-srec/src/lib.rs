//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

// Embed locale YAML files at compile time. Must be invoked at the crate root
// because the `t!` macro generates code that resolves `_rust_i18n_t` via
// `crate::_rust_i18n_t`. See `crate::i18n` for the wrapper module that
// re-exports `t!` and exposes locale-management helpers.
rust_i18n::i18n!("locales", fallback = "en");

pub mod api;
pub mod config;
pub mod credentials;
pub mod danmu;
pub mod database;
pub mod domain;
pub mod downloader;
pub mod error;
pub mod i18n;
pub mod logging;
pub mod metrics;
pub mod monitor;
pub mod notification;
pub mod panic_hook;
pub mod pipeline;
pub mod scheduler;
pub mod services;
pub mod session;
pub mod streamer;
pub mod utils;

pub use error::{Error, Result};
