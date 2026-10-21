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

    fn entry(id: &str) -> ProxyEntry {
        ProxyEntry {
            id: id.into(),
            name: format!("{id} name"),
            url: format!("http://{id}.example:8080"),
            username: Some("user".into()),
            password: Some("secret".into()),
            version: 1,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn the_most_specific_route_that_does_not_inherit_wins() {
        let entries = [entry("streamer"), entry("global")];
        let lookup = |id: &str| entries.iter().find(|entry| entry.id == id);
        let system = SystemProxy::default();
        let layers = |streamer: ProxyRoute, template: ProxyRoute, platform: ProxyRoute| {
            vec![
                (RouteSource::Streamer, streamer),
                (RouteSource::Template, template),
                (RouteSource::Platform, platform),
                (
                    RouteSource::Global,
                    ProxyRoute::Proxy {
                        id: "global".into(),
                    },
                ),
            ]
        };

        let resolved = resolve(
            &layers(
                ProxyRoute::Inherit,
                ProxyRoute::Inherit,
                ProxyRoute::Inherit,
            ),
            lookup,
            &system,
        )
        .unwrap();
        assert_eq!(resolved.source, RouteSource::Global);
        assert_eq!(
            resolved.key,
            RouteKey::Proxy {
                id: "global".into()
            }
        );
        assert_eq!(resolved.proxy_name(), Some("global name"));
        assert_eq!(
            resolved
                .target
                .endpoint()
                .and_then(|endpoint| endpoint.login()),
            Some(("user", "secret"))
        );

        let resolved = resolve(
            &layers(ProxyRoute::Inherit, ProxyRoute::Direct, ProxyRoute::System),
            lookup,
            &system,
        )
        .unwrap();
        assert_eq!(
            (resolved.target, resolved.source),
            (ProxyTarget::Direct, RouteSource::Template)
        );

        let resolved = resolve(
            &layers(
                ProxyRoute::Proxy {
                    id: "streamer".into(),
                },
                ProxyRoute::Direct,
                ProxyRoute::System,
            ),
            lookup,
            &system,
        )
        .unwrap();
        assert_eq!(resolved.source, RouteSource::Streamer);
        assert_eq!(
            resolved
                .target
                .endpoint()
                .map(|endpoint| endpoint.url.as_str()),
            Some("http://streamer.example:8080")
        );

        let resolved = resolve(
            &layers(ProxyRoute::Inherit, ProxyRoute::Inherit, ProxyRoute::System),
            lookup,
            &system,
        )
        .unwrap();
        assert_eq!(
            (resolved.target, resolved.source),
            (ProxyTarget::System, RouteSource::Platform)
        );

        assert_eq!(
            resolve(&[], lookup, &system).unwrap(),
            ResolvedRoute::direct(RouteSource::Global)
        );
    }

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
