//! Runtime bootstrap API for backend applications.
//!
//! This module is the supported integration surface for the server binary and
//! desktop shell. Application wiring remains internal so changes to routes,
//! services, and infrastructure do not expand the crate's public API.

pub use crate::api::auth_service::AuthResponse;
pub use crate::api::server::ApiServerConfig;
pub use crate::database::{
    init_database_pools, init_migration_pool, init_pool, init_write_pool, run_migrations,
};
pub use crate::downloader::{DownloadManagerConfig, DownloadManagerEvent};
pub use crate::logging::init_logging;
pub use crate::notification::{NotificationEvent, NotificationPriority};
pub use crate::panic_hook::install as install_panic_hook;
pub use crate::pipeline::PipelineManagerConfig;
pub use crate::runtime::RuntimeLease;
pub use crate::server::run as run_server;
pub use crate::services::{ActiveRecordingSummary, ServiceContainer, ServiceContainerConfig};
pub use crate::utils::http_client::install_rustls_provider;

/// A shared HTTP client on the proxy the environment configures, for
/// requests that serve no platform when no [`ServiceContainer`] is running
/// to supply the global route. Callers set timeouts per request.
pub fn system_service_client() -> crate::Result<reqwest::Client> {
    Ok(crate::utils::http_client::service_client(
        &crate::proxies::ProxyTarget::System,
    )?)
}
