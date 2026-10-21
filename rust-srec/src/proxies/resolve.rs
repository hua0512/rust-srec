//! Turning stored routes into the connection a request uses.

use platforms_parser::proxy::ProxyTarget;

use super::{
    ProxyEndpoint, ProxyError, ProxyName, ProxyRoute, ResolvedRoute, RouteKey, RouteSource,
    SystemProxy,
};

/// A saved proxy as stored. `Debug` never shows the login.
#[derive(Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProxyEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl std::fmt::Debug for ProxyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyEntry")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("authenticated", &self.username.is_some())
            .finish()
    }
}

impl ProxyEntry {
    pub fn endpoint(&self) -> ProxyEndpoint {
        ProxyEndpoint {
            url: self.url.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }
}

/// The connection `route` makes, credited to `source`; `None` when it
/// inherits. A route naming a missing entry is an error, never a direct
/// connection.
pub fn materialize(
    route: &ProxyRoute,
    source: RouteSource,
    entry: Option<&ProxyEntry>,
    system: &SystemProxy,
) -> Result<Option<ResolvedRoute>, ProxyError> {
    Ok(Some(match route {
        ProxyRoute::Inherit => return Ok(None),
        ProxyRoute::Direct => ResolvedRoute::direct(source),
        ProxyRoute::System => ResolvedRoute::system(source, system),
        ProxyRoute::Proxy { id } => {
            let entry = entry
                .filter(|entry| entry.id == *id)
                .ok_or_else(|| ProxyError::Missing(id.clone()))?;
            ResolvedRoute {
                target: ProxyTarget::Explicit(entry.endpoint()),
                proxy: Some(ProxyName {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                }),
                source,
                key: RouteKey::Proxy {
                    id: entry.id.clone(),
                },
            }
        }
    }))
}

/// The first route that does not inherit, most specific first, with
/// `lookup` supplying the entry a route names. When every layer inherits the
/// connection is direct.
pub fn resolve<'a>(
    layers: &[(RouteSource, ProxyRoute)],
    lookup: impl Fn(&str) -> Option<&'a ProxyEntry>,
    system: &SystemProxy,
) -> Result<ResolvedRoute, ProxyError> {
    for (source, route) in layers {
        let entry = route.proxy_id().and_then(&lookup);
        if let Some(resolved) = materialize(route, *source, entry, system)? {
            return Ok(resolved);
        }
    }
    Ok(ResolvedRoute::direct(RouteSource::Global))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_entry_fails_instead_of_connecting_directly() {
        let error = resolve(
            &[(
                RouteSource::Platform,
                ProxyRoute::Proxy { id: "gone".into() },
            )],
            |_| None,
            &SystemProxy::default(),
        )
        .unwrap_err();
        assert!(matches!(error, ProxyError::Missing(id) if id == "gone"));
    }
}
