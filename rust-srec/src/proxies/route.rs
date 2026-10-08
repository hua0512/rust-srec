//! How a scope connects, and the connection a request resolves to.

use platforms_parser::danmaku::{DanmakuError, DanmuProxy};
use platforms_parser::proxy::ProxyTarget;
use serde::{Deserialize, Serialize};

use super::{ProxyError, SystemProxy};

/// The connection a scope chooses. Platforms, templates and streamers may
/// inherit from the next scope out; global settings always decide. An
/// account that inherits follows the route of the operation using it.
///
/// Requests must not carry fields a route does not have: a misspelt route
/// would otherwise be read as another one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "RouteWire")]
pub enum ProxyRoute {
    #[default]
    Inherit,
    /// Connect without any proxy, ignoring proxy environment variables.
    Direct,
    /// Use the proxy the environment configures, if any.
    System,
    /// Connect through a saved proxy.
    Proxy { id: String },
}

/// The wire form a route is read from.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteWire {
    kind: String,
    #[serde(default)]
    id: Option<String>,
}

impl TryFrom<RouteWire> for ProxyRoute {
    type Error = String;

    fn try_from(wire: RouteWire) -> Result<Self, Self::Error> {
        match (wire.kind.as_str(), wire.id) {
            ("proxy", Some(id)) if !id.trim().is_empty() => Ok(Self::Proxy { id }),
            ("proxy", _) => Err("a proxy route needs the saved proxy's id".to_owned()),
            (kind @ ("inherit" | "direct" | "system"), Some(_)) => {
                Err(format!("a {kind} route has no id"))
            }
            ("inherit", None) => Ok(Self::Inherit),
            ("direct", None) => Ok(Self::Direct),
            ("system", None) => Ok(Self::System),
            (kind, _) => Err(format!(
                "unknown proxy route kind {kind:?}; expected inherit, direct, system or proxy"
            )),
        }
    }
}

impl ProxyRoute {
    /// The route stored in a scope's `proxy_route` and `proxy_id` columns.
    pub fn from_columns(kind: &str, id: Option<&str>) -> Result<Self, ProxyError> {
        match (kind, id) {
            ("inherit", None) => Ok(Self::Inherit),
            ("direct", None) => Ok(Self::Direct),
            ("system", None) => Ok(Self::System),
            ("proxy", Some(id)) => Ok(Self::Proxy { id: id.to_owned() }),
            _ => Err(ProxyError::invalid("stored proxy route is malformed")),
        }
    }

    /// The `proxy_route` column value.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Direct => "direct",
            Self::System => "system",
            Self::Proxy { .. } => "proxy",
        }
    }

    /// The `proxy_id` column value.
    pub fn proxy_id(&self) -> Option<&str> {
        match self {
            Self::Proxy { id } => Some(id),
            _ => None,
        }
    }

    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }
}

/// The network path a request takes, which platforms throttle. Requests
/// through the same saved proxy share a path whatever scope chose it; the
/// system route shares the direct path when no environment proxy is set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteKey {
    Direct,
    System,
    Proxy { id: String },
}

/// Which setting decided a request's route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Account,
    Streamer,
    Template,
    Platform,
    Global,
}

/// How a resolved route connects, as the API reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Direct,
    System,
    Proxy,
}

impl RouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::System => "system",
            Self::Proxy => "proxy",
        }
    }
}

/// A saved proxy as routes and notifications name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProxyName {
    pub id: String,
    pub name: String,
}

/// The key proxy names are unique and looked up by: names compare trimmed
/// and without case.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The connection a request uses, fixed when it is resolved: later edits to
/// the entry apply to the next resolution. `Debug` never shows a proxy login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// How HTTP clients, download engines and helper processes connect.
    pub target: ProxyTarget,
    /// The saved proxy, for an explicit target.
    pub proxy: Option<ProxyName>,
    pub source: RouteSource,
    pub key: RouteKey,
}

impl Default for ResolvedRoute {
    fn default() -> Self {
        Self::direct(RouteSource::Global)
    }
}

impl ResolvedRoute {
    pub fn direct(source: RouteSource) -> Self {
        Self {
            target: ProxyTarget::Direct,
            proxy: None,
            source,
            key: RouteKey::Direct,
        }
    }

    pub fn system(source: RouteSource, system: &SystemProxy) -> Self {
        Self {
            target: ProxyTarget::System,
            proxy: None,
            source,
            key: if system.detected() {
                RouteKey::System
            } else {
                RouteKey::Direct
            },
        }
    }

    pub fn kind(&self) -> RouteKind {
        match self.target {
            ProxyTarget::Direct => RouteKind::Direct,
            ProxyTarget::System => RouteKind::System,
            ProxyTarget::Explicit(_) => RouteKind::Proxy,
        }
    }

    /// The saved proxy's name, for logs and notifications.
    pub fn proxy_name(&self) -> Option<&str> {
        self.proxy.as_ref().map(|proxy| proxy.name.as_str())
    }

    /// The proxy danmu connections use. Danmu has no access to the
    /// operating system's proxy settings, so the system route uses the
    /// proxy environment variables, honouring `NO_PROXY`, and is direct
    /// when none is set.
    pub fn danmu_proxy(&self, system: &SystemProxy) -> Result<Option<DanmuProxy>, DanmakuError> {
        match &self.target {
            ProxyTarget::Direct => Ok(None),
            ProxyTarget::System => system.danmu_proxy(),
            ProxyTarget::Explicit(endpoint) => DanmuProxy::new(endpoint.clone()).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip_through_their_columns_and_json() {
        for route in [
            ProxyRoute::Inherit,
            ProxyRoute::Direct,
            ProxyRoute::System,
            ProxyRoute::Proxy { id: "p1".into() },
        ] {
            assert_eq!(
                ProxyRoute::from_columns(route.kind(), route.proxy_id()).unwrap(),
                route
            );
            let json = serde_json::to_value(&route).unwrap();
            assert_eq!(json["kind"], route.kind());
            assert_eq!(serde_json::from_value::<ProxyRoute>(json).unwrap(), route);
        }
        assert!(ProxyRoute::from_columns("proxy", None).is_err());
        assert!(ProxyRoute::from_columns("direct", Some("p1")).is_err());
        assert!(
            serde_json::from_value::<ProxyRoute>(serde_json::json!({"kind": "proxy"})).is_err()
        );
        assert!(
            serde_json::from_value::<ProxyRoute>(
                serde_json::json!({"kind": "direct", "url": "http://x"})
            )
            .is_err()
        );
    }

    #[test]
    fn the_system_route_shares_the_direct_path_without_an_environment_proxy() {
        let none = SystemProxy::default();
        let set = SystemProxy::from_lookup(|name| {
            (name == "HTTPS_PROXY").then(|| "http://proxy.example:3128".to_owned())
        });
        assert_eq!(
            ResolvedRoute::system(RouteSource::Global, &none).key,
            RouteKey::Direct
        );
        assert_eq!(
            ResolvedRoute::system(RouteSource::Global, &set).key,
            RouteKey::System
        );
    }

    #[test]
    fn danmu_follows_the_route() {
        let none = SystemProxy::default();
        let set = SystemProxy::from_lookup(|name| match name {
            "HTTPS_PROXY" => Some("proxy.example:3128".to_owned()),
            "NO_PROXY" => Some("internal.example".to_owned()),
            _ => None,
        });
        assert!(
            ResolvedRoute::direct(RouteSource::Global)
                .danmu_proxy(&set)
                .unwrap()
                .is_none()
        );
        let system = ResolvedRoute::system(RouteSource::Global, &set);
        assert!(system.danmu_proxy(&none).unwrap().is_none());
        let proxy = system.danmu_proxy(&set).unwrap().unwrap();
        assert!(proxy.bypasses("chat.internal.example"));
        assert!(!proxy.bypasses("danmu.example"));

        let explicit = ResolvedRoute {
            target: ProxyTarget::Explicit(platforms_parser::proxy::ProxyEndpoint::new(
                "socks5h://proxy.example:1080",
                Some(("user".into(), "p@ss".into())),
            )),
            ..ResolvedRoute::default()
        };
        let proxy = explicit.danmu_proxy(&none).unwrap().unwrap();
        assert!(!proxy.bypasses("danmu.example"));
        assert_eq!(
            format!("{proxy:?}"),
            "DanmuProxy(socks5h://proxy.example:1080)"
        );
    }
}
