//! Routes of the backend's own traffic that serves no platform.

use sqlx::SqlitePool;

use crate::Result;
use crate::database::repositories::proxies;
use crate::utils::http_client::service_client;

use super::{ProxyRoute, ProxyTarget, SystemProxy};

/// Resolves the connection of traffic that serves no platform, such as
/// notifications and browser push: its own route, else the global route,
/// with the saved proxy as it is stored at that moment. Platform, template
/// and streamer routes do not apply.
///
/// It holds only the database pool, so services using it keep no reference
/// to the rest of the proxy runtime.
#[derive(Clone)]
pub struct ServiceRoutes {
    pool: SqlitePool,
}

impl ServiceRoutes {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The connection a route `own` takes now. A route naming a saved proxy
    /// that no longer exists fails rather than connecting directly.
    pub async fn target(&self, own: &ProxyRoute) -> Result<ProxyTarget> {
        let mut connection = self.pool.acquire().await?;
        Ok(
            proxies::resolve_over_global(&mut connection, own, SystemProxy::current())
                .await?
                .target,
        )
    }

    /// A shared HTTP client on the connection `own` takes now; callers set
    /// timeouts per request.
    pub async fn client(&self, own: &ProxyRoute) -> Result<reqwest::Client> {
        Ok(service_client(&self.target(own).await?)?)
    }
}
