//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

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
pub use srec_notify::i18n;
#[doc(hidden)]
pub mod logging;
#[doc(hidden)]
pub mod metrics;
#[doc(hidden)]
pub mod monitor;
#[doc(hidden)]
pub use srec_notify as notification;
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

impl From<srec_notify::NotifyError> for Error {
    fn from(error: srec_notify::NotifyError) -> Self {
        match error {
            srec_notify::NotifyError::Database(e) => Self::from(e),
            srec_notify::NotifyError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_notify::NotifyError::Serialization(e) => Self::Serialization(e),
            srec_notify::NotifyError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_notify::NotifyError::Validation(msg) => Self::Validation(msg),
            srec_notify::NotifyError::Configuration(msg) => Self::Configuration(msg),
            srec_notify::NotifyError::Proxy(e) => Self::Proxy(e),
            srec_notify::NotifyError::NotificationDelivery { channel, message } => {
                Self::NotificationDelivery { channel, message }
            }
            srec_notify::NotifyError::Other(msg) => Self::Other(msg),
        }
    }
}
