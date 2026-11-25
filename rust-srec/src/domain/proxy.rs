//! Pure proxy value types, route resolution, error definitions, and
//! process-environment helpers shared across storage, downloaders, and upload
//! processors.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use platforms_parser::danmaku::{DanmakuError, DanmuProxy};
use platforms_parser::proxy::redacted_url;
pub use platforms_parser::proxy::{ProxyEndpoint, ProxyTarget};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::Result;
use crate::domain::step_routes::StepUser;

// ============================================================================
// Endpoint rules
// ============================================================================

/// Schemes every HTTP client, download engine and danmu tunnel can use.
pub(crate) const SUPPORTED_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

/// Login limits, in characters like every other limit the API states.
const MAX_USERNAME_CHARS: usize = 256;
const MAX_PASSWORD_CHARS: usize = 1024;

/// Rejects anything a client would not route through the proxy: the
/// shared client builder would otherwise connect directly.
pub(crate) fn validate(endpoint: &ProxyEndpoint) -> std::result::Result<(), ProxyError> {
    let url = url::Url::parse(endpoint.url.trim())
        .map_err(|_| ProxyError::invalid("proxy URL must be a valid URL"))?;
    if !SUPPORTED_SCHEMES.contains(&url.scheme()) {
        return Err(ProxyError::invalid(
            "proxy URL must use http, https, socks5 or socks5h",
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ProxyError::invalid("proxy URL must name a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProxyError::invalid(
            "put the proxy login in the username and password fields",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err(ProxyError::invalid(
            "proxy URL must not have a path or query",
        ));
    }
    // Clients and download engines send a proxy login only as a pair.
    match (&endpoint.username, &endpoint.password) {
        (Some(username), Some(password)) if !username.is_empty() => {
            if username.chars().count() > MAX_USERNAME_CHARS
                || password.chars().count() > MAX_PASSWORD_CHARS
            {
                return Err(ProxyError::invalid("proxy login is too long"));
            }
            if username
                .chars()
                .chain(password.chars())
                .any(char::is_control)
            {
                return Err(ProxyError::invalid(
                    "proxy login must not contain control characters",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ProxyError::invalid(
                "a proxy login needs both a username and a password",
            ));
        }
    }
    reqwest::Proxy::all(endpoint.url.trim())
        .map_err(|_| ProxyError::invalid("proxy URL is not supported"))?;
    Ok(())
}

/// The validated endpoint in its stored form: `scheme://host[:port]`
/// with a lowercase host, so equal exits compare equal.
pub fn canonical(endpoint: &ProxyEndpoint) -> std::result::Result<ProxyEndpoint, ProxyError> {
    validate(endpoint)?;
    let url = url::Url::parse(endpoint.url.trim())
        .map_err(|_| ProxyError::invalid("proxy URL must be a valid URL"))?;
    Ok(ProxyEndpoint {
        url: canonical_url(&url),
        username: endpoint.username.clone(),
        password: endpoint.password.clone(),
    })
}

pub(crate) fn host_text(url: &url::Url) -> String {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.to_ascii_lowercase(),
        Some(url::Host::Ipv4(address)) => address.to_string(),
        Some(url::Host::Ipv6(address)) => format!("[{address}]"),
        None => String::new(),
    }
}

pub(crate) fn canonical_url(url: &url::Url) -> String {
    let host = host_text(url);
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

// ============================================================================
// Errors and reference summaries
// ============================================================================

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
    pub notification_channels: Vec<NamedReference>,
    /// Job presets whose upload step chooses the entry.
    pub job_presets: Vec<NamedReference>,
    /// Configurations and workflows whose pipelines have an inline upload
    /// step choosing the entry.
    pub pipelines: Vec<StepUser>,
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
            + self.notification_channels.len()
            + self.job_presets.len()
            + self.pipelines.len()
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
        parts.extend(
            self.notification_channels
                .iter()
                .map(|channel| format!("notification channel {:?}", channel.name)),
        );
        parts.extend(
            self.job_presets
                .iter()
                .map(|preset| format!("job preset {:?}", preset.name)),
        );
        parts.extend(
            self.pipelines
                .iter()
                .map(|owner| format!("a pipeline of {owner}")),
        );
        f.write_str(&parts.join(", "))
    }
}

/// A platform, streamer, notification channel or job preset whose route
/// names the entry.
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

// ============================================================================
// System environment proxy snapshot
// ============================================================================

/// Variables naming the proxy, in the order they are consulted: an HTTPS
/// proxy first, as platforms are reached over HTTPS, then the catch-all, then
/// the plain HTTP proxy. Lowercase spellings win, as in curl.
const PROXY_VARIABLES: [&str; 6] = [
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "http_proxy",
    "HTTP_PROXY",
];
const NO_PROXY_VARIABLES: [&str; 2] = ["no_proxy", "NO_PROXY"];

/// Whether reqwest's system proxy lookup includes operating-system settings.
const OS_PROXY_SETTINGS: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// The environment's proxy. `Debug` never shows a login.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SystemProxy {
    url: Option<String>,
    no_proxy: Option<String>,
    /// Every proxy variable that is set, in [`PROXY_VARIABLES`] order; a
    /// helper process may read any of them.
    urls: Vec<String>,
}

impl std::fmt::Debug for SystemProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemProxy")
            .field("url", &self.url.as_deref().map(redacted_url))
            .field("no_proxy", &self.no_proxy)
            .finish()
    }
}

/// What the API shows of the environment's proxy.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SystemProxySummary {
    pub detected: bool,
    /// `scheme://host[:port]`, without any login.
    pub url: Option<String>,
    pub authenticated: bool,
    pub no_proxy: Option<String>,
}

impl SystemProxy {
    /// The snapshot taken from this process's environment on first use.
    pub fn current() -> &'static Self {
        static CURRENT: OnceLock<SystemProxy> = OnceLock::new();
        CURRENT.get_or_init(|| Self::from_lookup(|name| std::env::var(name).ok()))
    }

    /// Reads the variables through `lookup`, so callers can supply an
    /// environment other than the process's.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let set = |names: &[&str]| -> Vec<String> {
            names
                .iter()
                .filter_map(|name| lookup(name))
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .collect()
        };
        let urls: Vec<String> = set(&PROXY_VARIABLES)
            .into_iter()
            .map(|url| {
                if url.contains("://") {
                    url
                } else {
                    format!("http://{url}")
                }
            })
            .collect();
        Self {
            url: urls.first().cloned(),
            no_proxy: set(&NO_PROXY_VARIABLES).into_iter().next(),
            urls,
        }
    }

    pub fn detected(&self) -> bool {
        self.url.is_some()
    }

    /// Whether an HTTP client on `target` is certain to dial every destination
    /// itself, so it can drop proxy handling and check the addresses a
    /// destination resolves to as it connects.
    pub fn connects_directly(&self, target: &ProxyTarget) -> bool {
        match target {
            ProxyTarget::Direct => true,
            ProxyTarget::System => !OS_PROXY_SETTINGS && !self.detected(),
            ProxyTarget::Explicit(_) => false,
        }
    }

    pub fn summary(&self) -> SystemProxySummary {
        SystemProxySummary {
            detected: self.detected(),
            url: self.url.as_deref().map(redacted_url),
            authenticated: self
                .url
                .as_deref()
                .and_then(|url| url::Url::parse(url).ok())
                .is_some_and(|url| !url.username().is_empty()),
            no_proxy: self.no_proxy.clone(),
        }
    }

    /// The tunnel raw connections (danmu, Bigo's native stream) take on
    /// `target`; `None` connects directly.
    pub fn danmu_proxy_for(
        &self,
        target: &ProxyTarget,
    ) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
        match target {
            ProxyTarget::Direct => Ok(None),
            ProxyTarget::System => self.danmu_proxy(),
            ProxyTarget::Explicit(endpoint) => DanmuProxy::new(endpoint.clone()).map(Some),
        }
    }

    /// Every proxy the environment's variables name, with its login.
    pub fn endpoints(&self) -> impl Iterator<Item = ProxyEndpoint> + '_ {
        self.urls.iter().map(|url| ProxyEndpoint::from_url(url))
    }

    /// The danmu tunnel for the environment's proxy; `None` when none is set.
    pub fn danmu_proxy(&self) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
        self.url
            .as_deref()
            .map(|url| {
                DanmuProxy::parse(url).map(|proxy| proxy.with_no_proxy(self.no_proxy.clone()))
            })
            .transpose()
    }
}

// ============================================================================
// Routes and materialization
// ============================================================================

/// The connection a scope chooses. Platforms, templates and streamers may
/// inherit from the next scope out; global settings always decide. An
/// account that inherits follows the route of the operation using it.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
    schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "RouteWire")]
#[schemars(!try_from, deny_unknown_fields)]
pub enum ProxyRoute {
    #[default]
    Inherit,
    /// Connect without any proxy, ignoring proxy environment variables.
    Direct,
    /// Use the proxy the environment configures, if any.
    System,
    /// Connect through a saved proxy.
    Proxy {
        #[schemars(regex(pattern = r"\S"))]
        id: String,
    },
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

    fn try_from(wire: RouteWire) -> std::result::Result<Self, Self::Error> {
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
    pub fn from_columns(kind: &str, id: Option<&str>) -> std::result::Result<Self, ProxyError> {
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

    /// The proxy danmu connections use.
    pub fn danmu_proxy(
        &self,
        system: &SystemProxy,
    ) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
        system.danmu_proxy_for(&self.target)
    }
}

/// A saved proxy entry. `Debug` never shows the login.
#[derive(Clone, PartialEq, Eq)]
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
) -> std::result::Result<Option<ResolvedRoute>, ProxyError> {
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
) -> std::result::Result<ResolvedRoute, ProxyError> {
    for (source, route) in layers {
        let entry = route.proxy_id().and_then(&lookup);
        if let Some(resolved) = materialize(route, *source, entry, system)? {
            return Ok(resolved);
        }
    }
    Ok(ResolvedRoute::direct(RouteSource::Global))
}

// ============================================================================
// Upload-tool proxy port and output scrubbing
// ============================================================================

/// Resolves a [`ProxyRoute`] to a [`ProxyTarget`] for services and upload tools
/// that serve no platform (their own route, else the global route).
#[async_trait]
pub trait ProxyRouteResolver: Send + Sync {
    async fn target(&self, own: &ProxyRoute) -> Result<ProxyTarget>;
}

/// The connection an upload tool (rclone, BaiduPCS-Go) whose own route is
/// `route` takes now: that route, else the global route. Without `routes`,
/// as in unit tests of the processors, the tool keeps the server's
/// environment.
pub async fn upload_target(
    routes: Option<&dyn ProxyRouteResolver>,
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
/// proxy's and the system proxy's, as written and percent-encoded.
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

    fn endpoint(url: &str, login: Option<(&str, &str)>) -> ProxyEndpoint {
        ProxyEndpoint::new(
            url,
            login.map(|(user, pass)| (user.to_owned(), pass.to_owned())),
        )
    }

    fn environment(pairs: &[(&str, &str)]) -> SystemProxy {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        SystemProxy::from_lookup(move |name| {
            pairs
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone())
        })
    }

    #[test]
    fn only_proxies_every_client_routes_through_are_accepted() {
        for url in [
            "http://proxy.example:8080",
            "https://proxy.example",
            "socks5://127.0.0.1:1080",
            "socks5h://proxy.example:1080/",
            "http://[::1]:3128",
        ] {
            assert!(validate(&endpoint(url, None)).is_ok(), "{url}");
            assert!(
                validate(&endpoint(url, Some(("user", "")))).is_ok(),
                "{url}"
            );
        }
        for url in [
            "",
            "proxy.example:8080",
            "ftp://proxy.example",
            "socks4://proxy.example:1080",
            "http://user:secret@proxy.example:8080",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?session=1",
        ] {
            assert!(validate(&endpoint(url, None)).is_err(), "{url}");
        }
        let mut half = endpoint("http://proxy.example", Some(("user", "secret")));
        half.password = None;
        assert!(validate(&half).is_err());
        half.username = None;
        half.password = Some("secret".into());
        assert!(validate(&half).is_err());
        assert!(
            validate(&endpoint(
                "http://proxy.example",
                Some(("user\r\nX: y", "secret"))
            ))
            .is_err()
        );
    }

    #[test]
    fn equal_exits_have_one_canonical_form() {
        for (input, expected) in [
            ("HTTP://Proxy.Example:8080/", "http://proxy.example:8080"),
            ("http://proxy.example:80", "http://proxy.example"),
            (
                "socks5h://Proxy.Example:1080",
                "socks5h://proxy.example:1080",
            ),
            ("http://[0:0::1]:3128", "http://[::1]:3128"),
        ] {
            assert_eq!(canonical(&endpoint(input, None)).unwrap().url, expected);
        }
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
            target: ProxyTarget::Explicit(ProxyEndpoint::new(
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

    #[test]
    fn system_route_directness_follows_env_and_os_proxy_support() {
        let none = SystemProxy::default();
        let set = environment(&[("HTTPS_PROXY", "http://proxy.example:3128")]);
        assert_eq!(
            none.connects_directly(&ProxyTarget::System),
            !OS_PROXY_SETTINGS
        );
        assert!(!set.connects_directly(&ProxyTarget::System));
        assert!(set.connects_directly(&ProxyTarget::Direct));
    }

    #[test]
    fn the_https_proxy_wins_and_bare_addresses_mean_http() {
        assert!(!environment(&[]).detected());
        assert!(!environment(&[("HTTPS_PROXY", "  ")]).detected());
        let proxy = environment(&[
            ("HTTP_PROXY", "http://plain.example:80"),
            ("ALL_PROXY", "socks5://all.example:1080"),
            ("https_proxy", "secure.example:3128"),
        ]);
        assert_eq!(
            proxy.summary().url.as_deref(),
            Some("http://secure.example:3128")
        );
        assert_eq!(
            environment(&[
                ("HTTP_PROXY", "http://plain.example:8080"),
                ("ALL_PROXY", "socks5://all.example:1080"),
            ])
            .summary()
            .url
            .as_deref(),
            Some("socks5://all.example:1080")
        );
    }

    #[test]
    fn the_summary_never_shows_the_login() {
        let proxy = environment(&[
            ("HTTPS_PROXY", "http://user:secret@proxy.example:3128"),
            ("NO_PROXY", "localhost,.internal"),
        ]);
        let summary = proxy.summary();
        assert!(summary.authenticated);
        assert_eq!(summary.no_proxy.as_deref(), Some("localhost,.internal"));
        let rendered = format!("{} {proxy:?}", serde_json::to_string(&summary).unwrap());
        assert!(!rendered.contains("secret") && !rendered.contains("user"));
    }

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
