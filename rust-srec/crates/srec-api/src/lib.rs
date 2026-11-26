//! Presentation layer crate for `rust-srec`.
//!
//! Owns the REST API, WebSocket/Protobuf transport, OpenAPI schema generation,
//! MCP server, authentication store/service, CQRS read queries, playback
//! contexts, dynamic logging, and health metrics.

pub mod api;
pub mod error;
pub mod logging;
mod mcp;
pub mod metrics;

pub use error::{ServerError, ServerResult};

pub(crate) use error::{ServerError as Error, ServerResult as Result};
pub(crate) use srec_core::{domain, utils};
pub(crate) use srec_notify as notification;
pub(crate) use srec_pipeline as pipeline;
pub(crate) use srec_processors::baidupcs;
pub(crate) use srec_recorder::{
    config, credentials, danmu, downloader, monitor, proxies, recorder, session, streamer,
};

pub(crate) mod database {
    #[cfg(any(test, feature = "test-support"))]
    pub use srec_sqlite::run_migrations;
    #[cfg(test)]
    pub use srec_sqlite::{CommittedWriter, committed_writer, filter_store, init_pool_with_size};
    pub use srec_sqlite::{begin_immediate, models, retry, time};

    pub mod repositories {
        pub use srec_pipeline::repositories::*;
        pub use srec_sqlite::repositories::*;
    }

    #[cfg(test)]
    pub mod test_support {
        pub use srec_pipeline::test_support::*;
    }
}
