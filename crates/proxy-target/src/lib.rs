//! How clients and helper processes reach an upstream server: directly,
//! through the environment's proxy, or through an explicit proxy.

use std::fmt;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Every proxy variable a process that connects directly must not inherit.
pub const PROXY_ENVIRONMENT: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// Bytes escaped in a login written into a proxy URL: everything except
/// ASCII letters, digits and `*-._`.
const LOGIN: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

/// `scheme://host[:port]` of a proxy URL, without its login, path or query,
/// or a placeholder when it does not parse. Safe to log.
pub fn redacted_url(url: &str) -> String {
    url::Url::parse(url.trim())
        .ok()
        .and_then(|url| {
            let host = url.host_str().filter(|host| !host.is_empty())?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{}://{host}:{port}", url.scheme()),
                None => format!("{}://{host}", url.scheme()),
            })
        })
        .unwrap_or_else(|| "[unparsed]".to_owned())
}

/// A proxy address with its optional login. `Debug` never shows the login.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyEndpoint {
    /// `http`, `https`, `socks5` or `socks5h` URL. A login belongs in the
    /// fields below.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyEndpoint")
            .field("url", &redacted_url(&self.url))
            .field("authenticated", &self.login().is_some())
            .finish()
    }
}

impl ProxyEndpoint {
    pub fn new(url: impl Into<String>, login: Option<(String, String)>) -> Self {
        let (username, password) =
            login.map_or((None, None), |(user, pass)| (Some(user), Some(pass)));
        Self {
            url: url.into(),
            username,
            password,
        }
    }

    /// An endpoint from a URL that may carry a percent-encoded login, as
    /// proxy environment variables do. The login moves into the fields; a
    /// URL that does not parse is kept as written for the client to reject.
    pub fn from_url(url: &str) -> Self {
        let url = url.trim();
        let Ok(mut parsed) = url::Url::parse(url) else {
            return Self::new(url, None);
        };
        if parsed.username().is_empty() && parsed.password().is_none() {
            return Self::new(url, None);
        }
        let decode = |part: &str| percent_decode_str(part).decode_utf8_lossy().into_owned();
        let login = (
            decode(parsed.username()),
            decode(parsed.password().unwrap_or_default()),
        );
        if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
            return Self::new(url, None);
        }
        Self::new(parsed.as_str(), Some(login))
    }

    /// The login sent to the proxy. Only a complete pair is sent.
    pub fn login(&self) -> Option<(&str, &str)> {
        self.username.as_deref().zip(self.password.as_deref())
    }

    /// The proxy for every request scheme of a `reqwest` client, with its
    /// login.
    pub fn reqwest_proxy(&self) -> Result<reqwest::Proxy, UnusableProxy> {
        // reqwest percent-decodes the login it finds in a proxy URL, for HTTP
        // and SOCKS alike, and `Proxy::basic_auth` stores its login in that
        // URL without escaping `%`. The login therefore goes in encoded.
        let url = self.url_with_login();
        let url = url.trim();
        // reqwest accepts a proxy URL of any scheme, then sends requests
        // without the proxy when it cannot speak that scheme. A URL without a
        // host is left to reqwest, which reads `host:port` as an `http` proxy.
        if url::Url::parse(url)
            .is_ok_and(|url| url.has_host() && !SUPPORTED_SCHEMES.contains(&url.scheme()))
        {
            return Err(UnsupportedProxyScheme.into());
        }
        reqwest::Proxy::all(url).map_err(|error| UnusableProxy::Url(error.without_url()))
    }

    /// The URL with the login percent-encoded into it, for clients and
    /// programs that take the proxy as one URL. A URL that does not parse is
    /// returned as written for the client to reject.
    pub fn url_with_login(&self) -> String {
        let Some((username, password)) = self.login() else {
            return self.url.clone();
        };
        let Ok(mut url) = url::Url::parse(self.url.trim()) else {
            return self.url.clone();
        };
        let username = utf8_percent_encode(username, LOGIN).to_string();
        let password = utf8_percent_encode(password, LOGIN).to_string();
        if url.set_username(&username).is_err() || url.set_password(Some(&password)).is_err() {
            return self.url.clone();
        }
        url.into()
    }
}

/// A `reqwest` proxy no request gets through: requests fail before leaving
/// the host. Clients use it when the proxy they were given is unusable, so
/// they fail rather than connect without it.
pub fn unroutable_proxy() -> reqwest::Proxy {
    reqwest::Proxy::custom(|_| Some("http://unroutable.invalid:9".to_owned()))
}

/// The proxy URL schemes a `reqwest` client is given, which are also the ones
/// Go's `net/http` accepts from the environment. Go resolves host names on a
/// `socks5` proxy, so `socks5h` is passed as `socks5`, which older Go
/// releases also read.
pub const SUPPORTED_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

/// A proxy of a scheme that the client or program given it cannot use.
#[derive(Debug, thiserror::Error)]
#[error("can only use http, https and SOCKS5 proxies; choose another proxy")]
pub struct UnsupportedProxyScheme;

/// Why an endpoint cannot be the proxy of a `reqwest` client. The message
/// never quotes the proxy URL, so it can be shown and logged.
#[derive(Debug, thiserror::Error)]
pub enum UnusableProxy {
    #[error(transparent)]
    Scheme(#[from] UnsupportedProxyScheme),
    /// The client's own rejection, held without the URL.
    #[error(transparent)]
    Url(reqwest::Error),
}

/// How requests reach an upstream server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProxyTarget {
    /// No proxy, ignoring the proxy environment variables too.
    Direct,
    /// The proxy the environment configures, if any, as each client or
    /// process reads it.
    System,
    /// Through this proxy.
    Explicit(ProxyEndpoint),
}

impl ProxyTarget {
    /// The explicit proxy, if any.
    pub fn endpoint(&self) -> Option<&ProxyEndpoint> {
        match self {
            Self::Explicit(endpoint) => Some(endpoint),
            Self::Direct | Self::System => None,
        }
    }

    /// Whether a program that speaks only `schemes` to a proxy can use this
    /// target. Direct and system targets always qualify; an explicit proxy
    /// does when its URL uses one of the schemes.
    pub fn supports_schemes(&self, schemes: &[&str]) -> bool {
        self.endpoint().is_none_or(|endpoint| {
            url::Url::parse(endpoint.url.trim()).is_ok_and(|url| schemes.contains(&url.scheme()))
        })
    }

    /// Removes the proxy environment variables from a process that connects
    /// directly, so it connects directly whatever the environment says.
    /// Other targets leave the environment alone: an explicit proxy is passed
    /// as an argument, which the programs prefer over the environment.
    pub fn apply_environment(&self, command: &mut tokio::process::Command) {
        if *self == Self::Direct {
            remove_proxy_environment(command);
        }
    }

    /// Sets up the proxy environment of a program that reads its proxy only
    /// from `HTTP_PROXY` and `HTTPS_PROXY`, as Go programs such as rclone and
    /// BaiduPCS-Go do. A direct target removes every proxy variable; an
    /// explicit proxy replaces them with those two, login included; the
    /// system target leaves the environment alone. A proxy of a scheme Go
    /// cannot read from there is refused, so the program never connects
    /// without it.
    pub fn set_environment(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<(), UnsupportedProxyScheme> {
        match self {
            Self::System => {}
            Self::Direct => remove_proxy_environment(command),
            Self::Explicit(endpoint) => {
                if !self.supports_schemes(&SUPPORTED_SCHEMES) {
                    return Err(UnsupportedProxyScheme);
                }
                let url = endpoint.url_with_login();
                let url = match url.strip_prefix("socks5h://") {
                    Some(rest) => format!("socks5://{rest}"),
                    None => url,
                };
                remove_proxy_environment(command);
                command.env("HTTP_PROXY", &url).env("HTTPS_PROXY", &url);
            }
        }
        Ok(())
    }
}

fn remove_proxy_environment(command: &mut tokio::process::Command) {
    for name in PROXY_ENVIRONMENT {
        command.env_remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_encoded_into_the_url_once() {
        let endpoint = ProxyEndpoint::new(
            "http://proxy.example:8080",
            Some(("us@r.name".into(), "p:ss% w+*".into())),
        );
        assert_eq!(
            endpoint.url_with_login(),
            "http://us%40r.name:p%3Ass%25%20w%2B*@proxy.example:8080/"
        );
        let parsed = url::Url::parse(&endpoint.url_with_login()).unwrap();
        let decode = |part: &str| percent_decode_str(part).decode_utf8().unwrap().into_owned();
        assert_eq!(decode(parsed.username()), "us@r.name");
        assert_eq!(decode(parsed.password().unwrap()), "p:ss% w+*");

        let anonymous = ProxyEndpoint::new("http://proxy.example:8080", None);
        assert_eq!(anonymous.url_with_login(), "http://proxy.example:8080");
        let half = ProxyEndpoint {
            password: None,
            ..endpoint.clone()
        };
        assert_eq!(half.login(), None);
        assert_eq!(half.url_with_login(), "http://proxy.example:8080");
        let unparsed = ProxyEndpoint::new("not a url", Some(("u".into(), "p".into())));
        assert_eq!(unparsed.url_with_login(), "not a url");
    }

    #[test]
    fn a_login_in_a_url_moves_into_the_fields() {
        let endpoint = ProxyEndpoint::from_url(" socks5h://us%40er:p%3Ass@proxy.example:1080 ");
        assert_eq!(endpoint.login(), Some(("us@er", "p:ss")));
        assert!(!endpoint.url.contains("us%40er") && !endpoint.url.contains('@'));
        assert_eq!(redacted_url(&endpoint.url), "socks5h://proxy.example:1080");
        let plain = ProxyEndpoint::from_url("http://proxy.example:3128");
        assert_eq!(plain.url, "http://proxy.example:3128");
        assert_eq!(plain.login(), None);
        assert_eq!(ProxyEndpoint::from_url("[bad").url, "[bad");
    }

    #[test]
    fn nothing_shown_carries_the_login() {
        let endpoint = ProxyEndpoint::new(
            "http://proxy.example:8080",
            Some(("session-1".into(), "secret".into())),
        );
        let shown = format!("{:?}", ProxyTarget::Explicit(endpoint));
        assert!(!shown.contains("secret") && !shown.contains("session-1"));
        assert!(shown.contains("http://proxy.example:8080"));
        assert_eq!(
            redacted_url("socks5h://user:secret@proxy.example:1080/x?y"),
            "socks5h://proxy.example:1080"
        );
        for unparsed in ["user:secret@[bad", "", "mailto:user@example.com"] {
            assert_eq!(redacted_url(unparsed), "[unparsed]", "{unparsed}");
        }
    }

    /// The variables `setup` sets (`Some`) and removes (`None`) on a command.
    fn environment(
        setup: impl FnOnce(&mut tokio::process::Command),
    ) -> Vec<(String, Option<String>)> {
        let mut command = tokio::process::Command::new("program");
        setup(&mut command);
        command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    /// What `envs` does to the variable `name`: `None` when it leaves it
    /// alone. Windows compares variable names without case.
    fn variable(envs: &[(String, Option<String>)], name: &str) -> Option<Option<String>> {
        envs.iter()
            .find(|(key, _)| {
                if cfg!(windows) {
                    key.eq_ignore_ascii_case(name)
                } else {
                    key == name
                }
            })
            .map(|(_, value)| value.clone())
    }

    #[test]
    fn helper_processes_get_the_proxy_environment_their_target_calls_for() {
        let explicit = |url: &str| {
            ProxyTarget::Explicit(ProxyEndpoint::new(
                url,
                Some(("us@r".into(), "p:ss".into())),
            ))
        };
        let http = explicit("http://proxy.example:8080");
        let removes_all = |envs: &[(String, Option<String>)]| {
            for name in PROXY_ENVIRONMENT {
                assert_eq!(variable(envs, name), Some(None), "{name}: {envs:?}");
            }
        };

        removes_all(&environment(|command| {
            ProxyTarget::Direct.apply_environment(command)
        }));
        removes_all(&environment(|command| {
            ProxyTarget::Direct.set_environment(command).unwrap()
        }));
        for target in [ProxyTarget::System, http.clone()] {
            assert!(environment(|command| target.apply_environment(command)).is_empty());
        }
        assert!(
            environment(|command| ProxyTarget::System.set_environment(command).unwrap()).is_empty()
        );

        // The two variables Go reads carry the proxy and every other proxy
        // variable is removed; on Windows the lower-case names are the same
        // variables.
        let exported = |target: &ProxyTarget, url: &str| {
            let envs = environment(|command| target.set_environment(command).unwrap());
            for name in PROXY_ENVIRONMENT {
                let set = name.eq_ignore_ascii_case("HTTP_PROXY")
                    || name.eq_ignore_ascii_case("HTTPS_PROXY");
                let expected = if set && (cfg!(windows) || name.ends_with("_PROXY")) {
                    Some(url.to_owned())
                } else {
                    None
                };
                assert_eq!(variable(&envs, name), Some(expected), "{name}: {envs:?}");
            }
        };
        exported(&http, "http://us%40r:p%3Ass@proxy.example:8080/");
        exported(
            &explicit("socks5h://proxy.example:1080"),
            "socks5://us%40r:p%3Ass@proxy.example:1080",
        );

        let unsupported = explicit("ftp://proxy.example:21");
        let mut command = tokio::process::Command::new("program");
        assert!(unsupported.set_environment(&mut command).is_err());
        assert_eq!(command.as_std().get_envs().count(), 0);
        assert!(ProxyTarget::Direct.supports_schemes(&["http"]));
        assert!(!explicit("socks5://proxy.example:1080").supports_schemes(&["http"]));
    }
}
