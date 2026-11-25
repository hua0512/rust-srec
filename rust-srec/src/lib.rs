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

impl From<srec_recorder::RecorderError> for Error {
    fn from(error: srec_recorder::RecorderError) -> Self {
        match error {
            srec_recorder::RecorderError::ManualSplit(e) => Self::ManualSplit(e),
            srec_recorder::RecorderError::SchedulerFeedbackBusy { streamer_id } => {
                Self::SchedulerFeedbackBusy { streamer_id }
            }
            srec_recorder::RecorderError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_recorder::RecorderError::FilterSnapshot(e) => Self::FilterSnapshot(e),
            srec_recorder::RecorderError::Database(msg) => Self::Database(msg),
            srec_recorder::RecorderError::Db(e) => Self::from(e),
            srec_recorder::RecorderError::Core(e) => Self::from(e),
            srec_recorder::RecorderError::Serialization(e) => Self::Serialization(e),
            srec_recorder::RecorderError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_recorder::RecorderError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_recorder::RecorderError::Validation(msg) => Self::Validation(msg),
            srec_recorder::RecorderError::Conflict(msg) => Self::Conflict(msg),
            srec_recorder::RecorderError::Configuration(msg) => Self::Configuration(msg),
            srec_recorder::RecorderError::Io(e) => Self::Io(e),
            srec_recorder::RecorderError::IoPath { op, path, source } => {
                Self::IoPath { op, path, source }
            }
            srec_recorder::RecorderError::Monitor(msg) => Self::Monitor(msg),
            srec_recorder::RecorderError::Extractor(e) => Self::Extractor(e),
            srec_recorder::RecorderError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_recorder::RecorderError::CredentialUnavailable(e) => {
                Self::CredentialUnavailable(e)
            }
            srec_recorder::RecorderError::Proxy(e) => Self::Proxy(e),
            srec_recorder::RecorderError::DanmakuError(e) => Self::DanmakuError(e),
            srec_recorder::RecorderError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_recorder::RecorderError::Other(msg) => Self::Other(msg),
        }
    }
}

impl From<srec_api::ServerError> for Error {
    fn from(error: srec_api::ServerError) -> Self {
        match error {
            srec_api::ServerError::ManualSplit(e) => Self::ManualSplit(e),
            srec_api::ServerError::SchedulerFeedbackBusy { streamer_id } => {
                Self::SchedulerFeedbackBusy { streamer_id }
            }
            srec_api::ServerError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_api::ServerError::FilterSnapshot(e) => Self::FilterSnapshot(e),
            srec_api::ServerError::Database(msg) => Self::Database(msg),
            srec_api::ServerError::Serialization(e) => Self::Serialization(e),
            srec_api::ServerError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_api::ServerError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_api::ServerError::Validation(msg) => Self::Validation(msg),
            srec_api::ServerError::Conflict(msg) => Self::Conflict(msg),
            srec_api::ServerError::DagAlreadyTerminal { dag_id } => {
                Self::DagAlreadyTerminal { dag_id }
            }
            srec_api::ServerError::Configuration(msg) => Self::Configuration(msg),
            srec_api::ServerError::Io(e) => Self::Io(e),
            srec_api::ServerError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_api::ServerError::Monitor(msg) => Self::Monitor(msg),
            srec_api::ServerError::Extractor(e) => Self::Extractor(e),
            srec_api::ServerError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_api::ServerError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            srec_api::ServerError::Proxy(e) => Self::Proxy(e),
            srec_api::ServerError::PipelineError(msg) => Self::PipelineError(msg),
            srec_api::ServerError::NonRetryablePipeline(msg) => Self::NonRetryablePipeline(msg),
            srec_api::ServerError::ApiError(msg) => Self::ApiError(msg),
            srec_api::ServerError::DanmakuError(e) => Self::DanmakuError(e),
            srec_api::ServerError::Other(msg) => Self::Other(msg),
            srec_api::ServerError::NotificationDelivery { channel, message } => {
                Self::NotificationDelivery { channel, message }
            }
            srec_api::ServerError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_api::ServerError::PresetInUse {
                change,
                kind,
                name,
                users,
            } => Self::PresetInUse {
                change,
                kind,
                name,
                users,
            },
        }
    }
}

impl From<Error> for srec_api::ServerError {
    fn from(error: Error) -> Self {
        match error {
            Error::ManualSplit(e) => Self::ManualSplit(e),
            Error::SchedulerFeedbackBusy { streamer_id } => {
                Self::SchedulerFeedbackBusy { streamer_id }
            }
            Error::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            Error::FilterSnapshot(e) => Self::FilterSnapshot(e),
            Error::Database(msg) => Self::Database(msg),
            Error::Serialization(e) => Self::Serialization(e),
            Error::InvalidStateTransition { from, to } => Self::InvalidStateTransition { from, to },
            Error::NotFound { entity_type, id } => Self::NotFound { entity_type, id },
            Error::Validation(msg) => Self::Validation(msg),
            Error::Conflict(msg) => Self::Conflict(msg),
            Error::DagAlreadyTerminal { dag_id } => Self::DagAlreadyTerminal { dag_id },
            Error::Configuration(msg) => Self::Configuration(msg),
            Error::Io(e) => Self::Io(e),
            Error::IoPath { op, path, source } => Self::IoPath { op, path, source },
            Error::Monitor(msg) => Self::Monitor(msg),
            Error::Extractor(e) => Self::Extractor(e),
            Error::CredentialProfile(e) => Self::CredentialProfile(e),
            Error::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            Error::Proxy(e) => Self::Proxy(e),
            Error::PipelineError(msg) => Self::PipelineError(msg),
            Error::NonRetryablePipeline(msg) => Self::NonRetryablePipeline(msg),
            Error::ApiError(msg) => Self::ApiError(msg),
            Error::DanmakuError(e) => Self::DanmakuError(e),
            Error::Other(msg) => Self::Other(msg),
            Error::NotificationDelivery { channel, message } => {
                Self::NotificationDelivery { channel, message }
            }
            Error::DuplicateUrl(url) => Self::DuplicateUrl(url),
            Error::PresetInUse {
                change,
                kind,
                name,
                users,
            } => Self::PresetInUse {
                change,
                kind,
                name,
                users,
            },
        }
    }
}
