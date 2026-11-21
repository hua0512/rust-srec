//! Routes of the backend's own traffic that serves no platform, and the
//! environment of the upload tools that take one.

use sqlx::SqlitePool;
use tokio::process::Command;

use crate::Result;
use crate::database::repositories::proxies;
use crate::utils::http_client::service_client;

use super::{ProxyEndpoint, ProxyRoute, ProxyTarget, SystemProxy};

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

/// The connection an upload tool (rclone, BaiduPCS-Go) whose own route is
/// `route` takes now: that route, else the global route. Without `routes`,
/// as in unit tests of the processors, the tool keeps the server's
/// environment.
pub async fn upload_target(
    routes: Option<&ServiceRoutes>,
    route: &ProxyRoute,
) -> Result<ProxyTarget> {
    match routes {
        Some(routes) => routes.target(route).await,
        None => Ok(ProxyTarget::System),
    }
}

/// Starts the upload tool `tool` on `target` through its proxy
/// environment. A proxy the tool cannot read from there fails the command
/// rather than letting it connect without the proxy.
pub fn set_tool_environment(command: &mut Command, target: &ProxyTarget, tool: &str) -> Result<()> {
    target
        .set_environment(command)
        .map_err(|error| crate::Error::Validation(format!("{tool} {error}")))
}

/// The proxy login `target` carries, as it may appear in a tool's output:
/// the password percent-encoded and as written.
pub fn proxy_secrets(target: &ProxyTarget) -> Vec<String> {
    target.endpoint().map(endpoint_secrets).unwrap_or_default()
}

/// The proxy logins [`set_tool_environment`] gave `command`, longest first,
/// for scrubbing what it prints.
pub fn environment_secrets(command: &Command) -> Vec<String> {
    let mut secrets: Vec<String> = command
        .as_std()
        .get_envs()
        .filter(|(name, _)| {
            name.eq_ignore_ascii_case("HTTP_PROXY") || name.eq_ignore_ascii_case("HTTPS_PROXY")
        })
        .filter_map(|(_, value)| value?.to_str())
        .flat_map(|url| endpoint_secrets(&ProxyEndpoint::from_url(url)))
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets
}

fn endpoint_secrets(endpoint: &ProxyEndpoint) -> Vec<String> {
    let Some((_, password)) = endpoint.login() else {
        return Vec::new();
    };
    let mut secrets = vec![password.to_owned()];
    if let Ok(url) = url::Url::parse(&endpoint.url_with_login())
        && let Some(encoded) = url.password()
        && encoded != password
    {
        secrets.insert(0, encoded.to_owned());
    }
    secrets
}
