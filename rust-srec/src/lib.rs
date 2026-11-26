//! rust-srec library crate.
//!
//! This module exposes the core functionality for integration testing.

pub mod backend;
pub mod error;
mod runtime;
mod server;

// Internal modules kept `pub` only so the integration tests in `tests/` can
// import them; `backend` is the supported public surface.
#[doc(hidden)]
pub use srec_api::api;
#[doc(hidden)]
pub use srec_core::domain;
#[doc(hidden)]
pub use srec_core::utils;
#[doc(hidden)]
pub use srec_processors::baidupcs;
#[doc(hidden)]
pub use srec_recorder::config;
#[doc(hidden)]
pub use srec_recorder::credentials;
#[doc(hidden)]
pub use srec_recorder::danmu;
#[doc(hidden)]
pub mod database {
    pub use srec_sqlite::{
        CommittedWriter, DbError, DbPool, DbResult, ImmediateTransaction, WritePool,
        begin_immediate, committed_writer, default_read_pool_size, error, filter_store,
        init_database_pools, init_migration_pool, init_pool, init_pool_with_size, init_write_pool,
        models, prepare_fresh_database, retry, run_marked_conversion, run_migrations, time,
    };

    pub mod repositories {
        pub use srec_pipeline::repositories::*;
        pub use srec_sqlite::repositories::*;
    }

    #[cfg(test)]
    pub mod test_support {
        pub use srec_pipeline::test_support::*;
        pub use srec_sqlite::test_support::*;
    }
}
#[doc(hidden)]
pub use srec_api::logging;
#[doc(hidden)]
pub use srec_api::metrics;
#[doc(hidden)]
pub use srec_notify as notification;
#[doc(hidden)]
pub use srec_notify::i18n;
#[doc(hidden)]
pub use srec_recorder::downloader;
#[doc(hidden)]
pub use srec_recorder::monitor;
#[doc(hidden)]
pub mod panic_hook;
#[doc(hidden)]
pub use srec_pipeline as pipeline;
#[doc(hidden)]
pub use srec_recorder::proxies;
#[doc(hidden)]
pub use srec_recorder::recorder;
#[doc(hidden)]
pub mod services;
#[doc(hidden)]
pub use srec_recorder::session;
#[doc(hidden)]
pub use srec_recorder::streamer;

#[doc(hidden)]
pub use api::auth_request::override_socket_revalidation_interval;
pub use error::{Error, Result};
