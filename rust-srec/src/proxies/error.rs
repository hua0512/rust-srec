//! Errors of saved proxies and the routes that name them.

use serde::Serialize;

/// Why a proxy entry or route could not be saved or used.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid proxy: {0}")]
    Invalid(String),
    #[error("a proxy named {0:?} already exists")]
    NameTaken(String),
    /// Another entry already reaches the same exit: same address and username.
    #[error("proxy {name:?} already uses this address and username")]
    DuplicateEndpoint { name: String },
    #[error("proxy version changed")]
    StaleVersion,
    #[error("proxy is used by {0}")]
    Referenced(Box<ProxyReferences>),
    /// A route names an entry that does not exist. Requests on that route
    /// fail instead of connecting directly.
    #[error("proxy {0} does not exist")]
    Missing(String),
    #[error("the global proxy route must be direct, system or a saved proxy")]
    GlobalInherit,
    /// A request set `proxy_config`, which is not accepted: scopes choose a
    /// `proxy_route`.
    #[error("proxy_config is no longer accepted; choose a proxy_route instead")]
    ConfigReplaced,
    /// No HTTP client could be built for the route, which happens only when
    /// the TLS backend cannot initialize.
    #[error("no HTTP client could be built for the proxy route")]
    ClientUnavailable,
}

impl ProxyError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// What keeps a proxy entry in use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ProxyReferences {
    /// The global route names the entry.
    pub global: bool,
    pub platforms: Vec<NamedReference>,
    pub templates: Vec<TemplateReference>,
    pub streamers: Vec<NamedReference>,
    pub accounts: Vec<AccountReference>,
}

impl ProxyReferences {
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// How many routes name the entry.
    pub fn count(&self) -> usize {
        usize::from(self.global)
            + self.platforms.len()
            + self.templates.len()
            + self.streamers.len()
            + self.accounts.len()
    }
}

impl std::fmt::Display for ProxyReferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.global {
            parts.push("global settings".to_owned());
        }
        parts.extend(
            self.platforms
                .iter()
                .map(|scope| format!("platform {:?}", scope.name)),
        );
        parts.extend(
            self.templates
                .iter()
                .map(|scope| format!("template {:?}", scope.name)),
        );
        parts.extend(
            self.streamers
                .iter()
                .map(|scope| format!("streamer {:?}", scope.name)),
        );
        parts.extend(
            self.accounts
                .iter()
                .map(|account| format!("account {:?} on {}", account.label, account.platform_name)),
        );
        f.write_str(&parts.join(", "))
    }
}

/// A platform or streamer whose route names the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct NamedReference {
    pub id: String,
    pub name: String,
}

/// A template whose route names the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct TemplateReference {
    pub id: String,
    pub name: String,
    /// The template was deleted and is kept only until the recordings using
    /// it finish; it releases the entry then.
    pub being_removed: bool,
}

/// An account whose own route names the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct AccountReference {
    pub id: String,
    pub label: String,
    pub platform_id: String,
    pub platform_name: String,
}
