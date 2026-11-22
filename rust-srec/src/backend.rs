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

/// The HTTP client for the desktop app's check for a newer release, chosen
/// from the global route of `container` as it is now. Callers set timeouts
/// per request.
///
/// A global route naming a saved proxy takes that proxy, through the shared
/// client; a saved proxy that no longer exists fails rather than connecting
/// another way. Every other case takes the system proxy: a global route of
/// Direct or System proxy, and no `container` (a remote-mode desktop, or a
/// built-in backend that failed before its services existed). The check
/// contacts GitHub for the app itself rather than for the recorder, and a new
/// installation's Direct records only that no proxy environment variable was
/// set on its first start, not that the operating system's proxy should be
/// bypassed.
///
/// The system-proxy client is built for each check rather than shared:
/// reqwest reads the environment and the operating system's proxy setting
/// when a client is built, so a shared client would keep using the proxy in
/// effect at its first use even after the user starts or stops a local proxy.
pub async fn release_check_client(
    container: Option<&ServiceContainer>,
) -> crate::Result<reqwest::Client> {
    use crate::proxies::{ProxyRoute, ProxyTarget};
    use crate::utils::http_client::{apply_proxy, build_client, service_client};

    let target = match container {
        Some(container) => {
            container
                .proxies
                .routes()
                .target(&ProxyRoute::Inherit)
                .await?
        }
        None => ProxyTarget::System,
    };
    match target {
        ProxyTarget::Explicit(_) => Ok(service_client(&target)?),
        ProxyTarget::Direct | ProxyTarget::System => Ok(build_client(apply_proxy(
            reqwest::Client::builder(),
            &ProxyTarget::System,
        ))?),
    }
}
