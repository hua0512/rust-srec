//! Routes of the backend's own traffic that serves no platform, and the
//! environment of the upload tools that take one.

use std::sync::Arc;

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

/// Hides proxy logins in what an upload tool on `target` prints: the saved
/// proxy's and the system proxy's, as written and percent-encoded. Tool
/// output is parsed as printed; only the text written to job logs or
/// returned by the API passes through it.
#[derive(Clone, Default)]
pub struct OutputScrubber {
    /// Longest first, so a secret containing another is replaced whole.
    secrets: Arc<[String]>,
}

impl OutputScrubber {
    pub fn for_target(target: &ProxyTarget) -> Self {
        Self::with_system(target, SystemProxy::current())
    }

    fn with_system(target: &ProxyTarget, system: &SystemProxy) -> Self {
        let mut secrets: Vec<String> = target
            .endpoint()
            .cloned()
            .into_iter()
            .chain(system.endpoints())
            .flat_map(|endpoint| endpoint_secrets(&endpoint))
            .collect();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self {
            secrets: secrets.into(),
        }
    }

    /// The logins it hides, longest first.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.secrets.iter().map(String::as_str)
    }

    pub fn scrub(&self, text: &str) -> String {
        if self.secrets.is_empty() {
            return text.to_owned();
        }
        let secrets: Vec<&str> = self.secrets().collect();
        crate::utils::text::scrub(text, &secrets)
    }
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
        secrets.push(encoded.to_owned());
    }
    secrets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_proxy_and_every_system_proxy_login_are_hidden() {
        let saved = ProxyTarget::Explicit(ProxyEndpoint::new(
            "http://saved.example:8080",
            Some(("user".into(), "saved:pw".into())),
        ));
        let system = SystemProxy::from_lookup(|name| match name {
            "HTTPS_PROXY" => Some("http://a:https%2Fpw@secure.example:3128".into()),
            "http_proxy" => Some("plain.example:3128".into()),
            "ALL_PROXY" => Some("socks5://b:all%40pw@all.example:1080".into()),
            _ => None,
        });
        let scrubber = OutputScrubber::with_system(&saved, &system);
        let output = "saved:pw saved%3Apw https/pw https%2Fpw all@pw all%40pw plain.example";
        assert_eq!(
            scrubber.scrub(output),
            "*** *** *** *** *** *** plain.example"
        );
    }
}
