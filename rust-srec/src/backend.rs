//! Runtime bootstrap API for backend applications.
//!
//! This module is the supported integration surface for the server binary and
//! desktop shell. Application wiring remains internal so changes to routes,
//! services, and infrastructure do not expand the crate's public API.

pub use crate::api::server::ApiServerConfig;
pub use crate::danmu::service::DanmuServiceConfig;
pub use crate::database::{init_pool, init_write_pool, run_migrations};
pub use crate::downloader::DownloadManagerConfig;
pub use crate::logging::init_logging;
pub use crate::notification::{NotificationEvent, NotificationPriority};
pub use crate::panic_hook::install as install_panic_hook;
pub use crate::pipeline::PipelineManagerConfig;
pub use crate::services::{ServiceContainer, ServiceContainerConfig, ServiceStats};
pub use crate::utils::http_client::install_rustls_provider;
