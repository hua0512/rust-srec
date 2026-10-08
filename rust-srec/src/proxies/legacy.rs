//! Reading the `proxy_config` JSON that scopes stored before saved proxies.

use std::collections::{HashMap, HashSet};

use percent_encoding::percent_decode_str;
use serde::Deserialize;

use super::endpoint::{SUPPORTED_SCHEMES, canonical_url, host_text, validate};
use super::{ProxyEndpoint, name_key};

/// The route a stored `proxy_config` meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRoute {
    Inherit,
    Direct,
    System,
    Proxy(ProxyEndpoint),
}

impl LegacyRoute {
    /// The route as a backup writes it, naming an entry for an address
    /// through `name_of`.
    pub fn into_backup(
        self,
        name_of: impl FnOnce(&ProxyEndpoint) -> String,
    ) -> crate::config::backup::BackupRoute {
        use crate::config::backup::BackupRoute;
        match self {
            Self::Inherit => BackupRoute::Inherit,
            Self::Direct => BackupRoute::Direct,
            Self::System => BackupRoute::System,
            Self::Proxy(endpoint) => BackupRoute::Proxy {
                name: name_of(&endpoint),
            },
        }
    }
}

/// A converted setting and, when it could not be kept as written, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMapping {
    pub route: LegacyRoute,
    pub warning: Option<&'static str>,
    /// The setting asked for no proxy: disabled, empty or unreadable. Only
    /// such a global setting may become the system route.
    pub unset: bool,
}

#[derive(Deserialize)]
struct StoredProxy {
    enabled: bool,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    use_system_proxy: bool,
}

impl LegacyMapping {
    fn new(route: LegacyRoute, warning: Option<&'static str>, unset: bool) -> Self {
        Self {
            route,
            warning,
            unset,
        }
    }
}

/// Maps a stored `proxy_config` value. Absent, empty and unreadable values
/// inherit, as they did; a disabled one is direct; an address wins over the
/// system flag; an address no client can use becomes direct with a warning.
pub fn map_value(value: Option<&serde_json::Value>) -> LegacyMapping {
    let value = value.map(|value| crate::config::backup::unwrap_json_value(value.clone()));
    let Some(value) = value.filter(|value| match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(text) => !text.trim().is_empty(),
        _ => true,
    }) else {
        return LegacyMapping::new(LegacyRoute::Inherit, None, true);
    };
    let Ok(stored) = serde_json::from_value::<StoredProxy>(value) else {
        return unreadable();
    };
    if !stored.enabled {
        return LegacyMapping::new(LegacyRoute::Direct, None, true);
    }
    let url = stored.url.as_deref().map(str::trim).unwrap_or_default();
    if url.is_empty() {
        return if stored.use_system_proxy {
            LegacyMapping::new(LegacyRoute::System, None, false)
        } else {
            LegacyMapping::new(LegacyRoute::Direct, None, true)
        };
    }
    match endpoint_from_legacy(url, stored.username.as_deref(), stored.password.as_deref()) {
        Ok(endpoint) => LegacyMapping::new(LegacyRoute::Proxy(endpoint), None, false),
        Err(LegacyRejection::Empty) => LegacyMapping::new(LegacyRoute::Direct, None, true),
        Err(LegacyRejection::Unusable(reason)) => {
            LegacyMapping::new(LegacyRoute::Direct, Some(reason), false)
        }
    }
}

/// Maps a stored `proxy_config` setting.
pub fn map_text(raw: Option<&str>) -> LegacyMapping {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        None => map_value(None),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(value) => map_value(Some(&value)),
            Err(_) => unreadable(),
        },
    }
}

/// Why a stored legacy proxy setting cannot become an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyRejection {
    /// No address was given.
    Empty,
    /// The address does not parse or uses a scheme no client supports.
    Unusable(&'static str),
}

/// An endpoint from a proxy setting written before saved proxies.
///
/// Those settings accepted a bare `host:port`, which every client read as
/// `http://`, and a login embedded in the URL. Login fields win over an
/// embedded login; a field login counts only as a complete pair, as it did
/// when it was sent.
fn endpoint_from_legacy(
    url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<ProxyEndpoint, LegacyRejection> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(LegacyRejection::Empty);
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|_| LegacyRejection::Unusable("the proxy URL does not parse"))?;
    if !SUPPORTED_SCHEMES.contains(&parsed.scheme()) {
        return Err(LegacyRejection::Unusable(
            "the proxy scheme is not http, https, socks5 or socks5h",
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(LegacyRejection::Unusable("the proxy URL names no host"));
    }
    let decode = |part: &str| percent_decode_str(part).decode_utf8_lossy().into_owned();
    let login = match (username, password) {
        (Some(username), Some(password)) if !username.is_empty() => {
            Some((username.to_owned(), password.to_owned()))
        }
        _ if !parsed.username().is_empty() => Some((
            decode(parsed.username()),
            decode(parsed.password().unwrap_or_default()),
        )),
        _ => None,
    };
    let endpoint = ProxyEndpoint::new(canonical_url(&parsed), login);
    validate(&endpoint).map_err(|_| LegacyRejection::Unusable("the proxy URL is not supported"))?;
    Ok(endpoint)
}

/// `host[:port]`, the name a converted entry starts from.
fn host_port(endpoint: &ProxyEndpoint) -> String {
    url::Url::parse(&endpoint.url).map_or_else(
        |_| endpoint.url.clone(),
        |url| {
            let host = host_text(&url);
            match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            }
        },
    )
}

fn unreadable() -> LegacyMapping {
    LegacyMapping::new(
        LegacyRoute::Inherit,
        Some("the proxy setting was unreadable and was ignored"),
        true,
    )
}

/// The global route: it cannot inherit. A global setting that asked for no
/// proxy left download engines on the environment's proxy, so with one set it
/// becomes the system route rather than switching those downloads to direct.
pub fn global_route(mapping: LegacyMapping, environment_proxy: bool) -> LegacyRoute {
    match mapping.route {
        LegacyRoute::Inherit | LegacyRoute::Direct if mapping.unset && environment_proxy => {
            LegacyRoute::System
        }
        LegacyRoute::Inherit => LegacyRoute::Direct,
        route => route,
    }
}

/// An entry a conversion uses, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedEndpoint {
    pub name: String,
    pub endpoint: ProxyEndpoint,
    /// Whether the conversion creates the entry; otherwise it already exists.
    pub created: bool,
}

/// Names converted entries after their address, `host:port`, adding ` 2`,
/// ` 3`… when that name is taken. Settings reaching the same exit (address
/// and username) share one entry: exits are unique.
#[derive(Debug, Default)]
pub struct Namer {
    taken: HashSet<String>,
    by_exit: HashMap<(String, Option<String>), usize>,
    entries: Vec<NamedEndpoint>,
    conflicts: usize,
}

impl Namer {
    /// Starts from the entries that already exist.
    pub fn new<'a>(existing: impl IntoIterator<Item = (&'a str, &'a ProxyEndpoint)>) -> Self {
        let mut namer = Self::default();
        for (name, endpoint) in existing {
            namer.taken.insert(name_key(name));
            namer.by_exit.insert(
                (endpoint.url.clone(), endpoint.username.clone()),
                namer.entries.len(),
            );
            namer.entries.push(NamedEndpoint {
                name: name.to_owned(),
                endpoint: endpoint.clone(),
                created: false,
            });
        }
        namer
    }

    /// The entry for `endpoint`, reusing one that reaches the same exit. A
    /// second password for the same exit cannot be kept: the first stands
    /// and the difference is counted in [`Self::password_conflicts`].
    pub fn entry_for(&mut self, endpoint: &ProxyEndpoint) -> &NamedEndpoint {
        let key = (endpoint.url.clone(), endpoint.username.clone());
        if let Some(index) = self.by_exit.get(&key).copied() {
            if self.entries[index].endpoint.password != endpoint.password {
                self.conflicts += 1;
            }
            return &self.entries[index];
        }
        let base = host_port(endpoint);
        let base: String = base.chars().take(56).collect();
        let name = std::iter::once(base.clone())
            .chain((2..).map(|suffix| format!("{base} {suffix}")))
            .find(|name| !self.taken.contains(&name_key(name)))
            .unwrap_or(base);
        self.taken.insert(name_key(&name));
        self.by_exit.insert(key, self.entries.len());
        self.entries.push(NamedEndpoint {
            name,
            endpoint: endpoint.clone(),
            created: true,
        });
        let index = self.entries.len() - 1;
        &self.entries[index]
    }

    /// Entries the conversion creates, in creation order.
    pub fn created(&self) -> impl Iterator<Item = &NamedEndpoint> {
        self.entries.iter().filter(|entry| entry.created)
    }

    /// Settings that reached an existing exit with another password.
    pub fn password_conflicts(&self) -> usize {
        self.conflicts
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn proxy(url: &str, login: Option<(&str, &str)>) -> LegacyRoute {
        LegacyRoute::Proxy(ProxyEndpoint::new(
            url,
            login.map(|(user, pass)| (user.to_owned(), pass.to_owned())),
        ))
    }

    #[test]
    fn stored_settings_map_to_the_route_they_meant() {
        let cases: Vec<(Option<serde_json::Value>, LegacyRoute, bool)> = vec![
            (None, LegacyRoute::Inherit, false),
            (Some(json!(null)), LegacyRoute::Inherit, false),
            (Some(json!("")), LegacyRoute::Inherit, false),
            (Some(json!({"url": "x"})), LegacyRoute::Inherit, true),
            (
                Some(json!({"enabled": false, "url": "http://p:1"})),
                LegacyRoute::Direct,
                false,
            ),
            (
                Some(
                    json!({"enabled": true, "url": "proxy.example:3128", "username": "u", "password": "p"}),
                ),
                proxy("http://proxy.example:3128", Some(("u", "p"))),
                false,
            ),
            (
                Some(
                    json!({"enabled": true, "url": "http://p.example:1", "use_system_proxy": true}),
                ),
                proxy("http://p.example:1", None),
                false,
            ),
            (
                Some(json!({"enabled": true, "url": " ", "use_system_proxy": true})),
                LegacyRoute::System,
                false,
            ),
            (Some(json!({"enabled": true})), LegacyRoute::Direct, false),
            (
                Some(json!({"enabled": true, "url": "socks4://p.example:1080"})),
                LegacyRoute::Direct,
                true,
            ),
            (
                // Backups double-encode JSON settings.
                Some(json!(
                    "{\"enabled\":true,\"url\":\"socks5h://p.example:1080\"}"
                )),
                proxy("socks5h://p.example:1080", None),
                false,
            ),
        ];
        for (value, route, warned) in cases {
            let mapping = map_value(value.as_ref());
            assert_eq!(mapping.route, route, "{value:?}");
            assert_eq!(mapping.warning.is_some(), warned, "{value:?}");
        }
        assert_eq!(map_text(Some("not json")).route, LegacyRoute::Inherit);
        assert!(map_text(Some("not json")).warning.is_some());
        assert_eq!(map_text(Some("")).route, LegacyRoute::Inherit);
        assert_eq!(
            map_text(Some(r#"{"enabled":false,"url":null}"#)).route,
            LegacyRoute::Direct
        );
    }

    #[test]
    fn legacy_settings_keep_the_address_they_were_read_as() {
        let bare = endpoint_from_legacy("Proxy.Example:3128", None, None).unwrap();
        assert_eq!(bare.url, "http://proxy.example:3128");
        assert_eq!(host_port(&bare), "proxy.example:3128");

        let embedded =
            endpoint_from_legacy("socks5://us%40er:p%3Ass@proxy.example:1080", None, None).unwrap();
        assert_eq!(embedded.url, "socks5://proxy.example:1080");
        assert_eq!(embedded.username.as_deref(), Some("us@er"));
        assert_eq!(embedded.password.as_deref(), Some("p:ss"));

        let fields = endpoint_from_legacy(
            "http://old:login@proxy.example:8080",
            Some("field"),
            Some("secret"),
        )
        .unwrap();
        assert_eq!(fields.username.as_deref(), Some("field"));
        assert_eq!(fields.password.as_deref(), Some("secret"));

        // A field username without a password was never sent.
        let half = endpoint_from_legacy("proxy.example:8080", Some("user"), None).unwrap();
        assert!(half.username.is_none() && half.password.is_none());

        assert_eq!(
            endpoint_from_legacy("  ", None, None),
            Err(LegacyRejection::Empty)
        );
        for unusable in [
            "socks4://proxy.example:1080",
            "http://",
            "not a url at all:x:y",
        ] {
            assert!(
                matches!(
                    endpoint_from_legacy(unusable, None, None),
                    Err(LegacyRejection::Unusable(_))
                ),
                "{unusable}"
            );
        }
    }

    #[test]
    fn global_settings_without_a_proxy_keep_the_environment_proxy() {
        for raw in [
            None,
            Some(""),
            Some("garbage"),
            Some(r#"{"enabled":false}"#),
            Some(r#"{"enabled":true}"#),
        ] {
            assert_eq!(
                global_route(map_text(raw), true),
                LegacyRoute::System,
                "{raw:?}"
            );
            assert_eq!(
                global_route(map_text(raw), false),
                LegacyRoute::Direct,
                "{raw:?}"
            );
        }
        let explicit = Some(r#"{"enabled":true,"url":"http://p.example:1"}"#);
        assert_eq!(
            global_route(map_text(explicit), true),
            proxy("http://p.example:1", None)
        );
        // An address no client can use was a request for a proxy, not for none.
        let unusable = Some(r#"{"enabled":true,"url":"socks4://p.example:1"}"#);
        assert_eq!(global_route(map_text(unusable), true), LegacyRoute::Direct);
        let system = Some(r#"{"enabled":true,"use_system_proxy":true}"#);
        assert_eq!(global_route(map_text(system), false), LegacyRoute::System);
    }

    #[test]
    fn converted_entries_share_exits_and_avoid_taken_names() {
        let existing = ProxyEndpoint::new("http://saved.example:1", None);
        let mut namer = Namer::new([("proxy.example:8080", &existing)]);
        let first = ProxyEndpoint::new(
            "http://proxy.example:8080",
            Some(("user".into(), "secret".into())),
        );
        assert_eq!(namer.entry_for(&first).name, "proxy.example:8080 2");
        let other_user = ProxyEndpoint::new(
            "http://proxy.example:8080",
            Some(("other".into(), "secret".into())),
        );
        assert_eq!(namer.entry_for(&other_user).name, "proxy.example:8080 3");
        assert_eq!(namer.entry_for(&first.clone()).name, "proxy.example:8080 2");
        let rotated = ProxyEndpoint::new(
            "http://proxy.example:8080",
            Some(("user".into(), "rotated".into())),
        );
        assert_eq!(namer.entry_for(&rotated).name, "proxy.example:8080 2");
        assert_eq!(namer.password_conflicts(), 1);
        let saved = namer.entry_for(&existing.clone()).clone();
        assert!(!saved.created);
        assert_eq!(namer.created().count(), 2);
    }
}
