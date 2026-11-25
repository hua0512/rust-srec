//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

// Embed locale YAML files at compile time. Must be invoked at the crate root
// because the `t!` macro generates code that resolves `_rust_i18n_t` via
// `crate::_rust_i18n_t`. See `crate::i18n` for the wrapper module that
// re-exports `t!` and exposes locale-management helpers.
rust_i18n::i18n!("locales", fallback = "en");

pub mod backend;
pub mod error;
mod mcp;
mod runtime;
mod server;

// Internal modules kept `pub` only so the integration tests in `tests/` can
// import them; `backend` is the supported public surface.
#[doc(hidden)]
pub mod api;
#[doc(hidden)]
pub use srec_processors::baidupcs;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod credentials;
#[doc(hidden)]
pub mod danmu;
#[doc(hidden)]
pub use srec_core::domain;
#[doc(hidden)]
pub use srec_core::utils;
#[doc(hidden)]
pub use srec_sqlite as database;
#[doc(hidden)]
pub mod downloader;
#[doc(hidden)]
pub mod i18n;
#[doc(hidden)]
pub mod logging;
#[doc(hidden)]
pub mod metrics;
#[doc(hidden)]
pub mod monitor;
#[doc(hidden)]
pub mod notification;
#[doc(hidden)]
pub mod panic_hook;
#[doc(hidden)]
pub mod pipeline;
#[doc(hidden)]
pub mod proxies;
#[doc(hidden)]
pub mod recorder;
#[doc(hidden)]
pub mod services;
#[doc(hidden)]
pub mod session;
#[doc(hidden)]
pub mod streamer;

#[doc(hidden)]
pub use api::auth_request::override_socket_revalidation_interval;
pub use error::{Error, Result};
