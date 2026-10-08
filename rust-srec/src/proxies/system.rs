//! The proxy the environment configures, read once at startup.
//!
//! In-process HTTP clients on the system route defer to their own environment
//! and operating-system lookup; download engines inherit the environment. This
//! snapshot decides what danmu connections use, how the system route is
//! throttled, and what the API reports as detected.

use std::sync::OnceLock;

use platforms_parser::danmaku::{DanmakuError, DanmuProxy};
use platforms_parser::proxy::redacted_url;
use serde::Serialize;

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

/// The environment's proxy. `Debug` never shows a login.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SystemProxy {
    url: Option<String>,
    no_proxy: Option<String>,
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
        let first = |names: &[&str]| {
            names
                .iter()
                .filter_map(|name| lookup(name))
                .map(|value| value.trim().to_owned())
                .find(|value| !value.is_empty())
        };
        Self {
            url: first(&PROXY_VARIABLES).map(|url| {
                if url.contains("://") {
                    url
                } else {
                    format!("http://{url}")
                }
            }),
            no_proxy: first(&NO_PROXY_VARIABLES),
        }
    }

    pub fn detected(&self) -> bool {
        self.url.is_some()
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

    /// The danmu tunnel for the environment's proxy; `None` when none is set.
    pub fn danmu_proxy(&self) -> Result<Option<DanmuProxy>, DanmakuError> {
        self.url
            .as_deref()
            .map(|url| {
                DanmuProxy::parse(url).map(|proxy| proxy.with_no_proxy(self.no_proxy.clone()))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
