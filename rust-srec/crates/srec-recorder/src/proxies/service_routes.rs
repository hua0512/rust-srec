//! Routes of the backend's own traffic that serves no platform.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use srec_core::domain::proxy::{ProxyRoute, ProxyRouteResolver, ProxyTarget, SystemProxy};
use srec_core::utils::http_client::service_client;
use srec_sqlite::repositories::proxies;

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
    /// that no longer exists fails rather than connecting directly. The
    /// global route and the saved proxy it names are read in one
    /// transaction, so a concurrent change replacing both is seen whole.
    pub async fn target(&self, own: &ProxyRoute) -> Result<ProxyTarget> {
        let mut transaction = self.pool.begin().await?;
        let target = proxies::resolve_over_global(&mut transaction, own, SystemProxy::current())
            .await?
            .target;
        transaction.commit().await?;
        Ok(target)
    }

    /// A shared HTTP client on the connection `own` takes now; callers set
    /// timeouts per request.
    pub async fn client(&self, own: &ProxyRoute) -> Result<reqwest::Client> {
        Ok(service_client(&self.target(own).await?)?)
    }
}

#[async_trait]
impl ProxyRouteResolver for ServiceRoutes {
    async fn target(&self, own: &ProxyRoute) -> srec_core::CoreResult<ProxyTarget> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|e| srec_core::CoreError::Database(e.to_string()))?;
        let target = proxies::resolve_over_global(&mut transaction, own, SystemProxy::current())
            .await?
            .target;
        transaction
            .commit()
            .await
            .map_err(|e| srec_core::CoreError::Database(e.to_string()))?;
        Ok(target)
    }
}
