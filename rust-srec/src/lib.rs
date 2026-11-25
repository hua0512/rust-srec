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
pub use srec_pipeline as pipeline;
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

impl From<srec_pipeline::PipelineError> for Error {
    fn from(error: srec_pipeline::PipelineError) -> Self {
        match error {
            srec_pipeline::PipelineError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_pipeline::PipelineError::Database(msg) => Self::Database(msg),
            srec_pipeline::PipelineError::Db(e) => Self::from(e),
            srec_pipeline::PipelineError::Core(e) => Self::from(e),
            srec_pipeline::PipelineError::Serialization(e) => Self::Serialization(e),
            srec_pipeline::PipelineError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_pipeline::PipelineError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_pipeline::PipelineError::Validation(msg) => Self::Validation(msg),
            srec_pipeline::PipelineError::Conflict(msg) => Self::Conflict(msg),
            srec_pipeline::PipelineError::DagAlreadyTerminal { dag_id } => {
                Self::DagAlreadyTerminal { dag_id }
            }
            srec_pipeline::PipelineError::Configuration(msg) => Self::Configuration(msg),
            srec_pipeline::PipelineError::Io(e) => Self::Io(e),
            srec_pipeline::PipelineError::IoPath { op, path, source } => {
                Self::IoPath { op, path, source }
            }
            srec_pipeline::PipelineError::Proxy(e) => Self::Proxy(e),
            srec_pipeline::PipelineError::PipelineError(msg) => Self::PipelineError(msg),
            srec_pipeline::PipelineError::NonRetryablePipeline(msg) => {
                Self::NonRetryablePipeline(msg)
            }
            srec_pipeline::PipelineError::Other(msg) => Self::Other(msg),
        }
    }
}
