//! How extractors, their helper processes and danmu connections reach a
//! platform: directly, through the environment's proxy, or through an
//! explicit proxy.

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
    /// login. The error may quote the URL, so callers should not show it.
    pub fn reqwest_proxy(&self) -> reqwest::Result<reqwest::Proxy> {
        // reqwest percent-decodes the login it finds in a proxy URL, for HTTP
        // and SOCKS alike, and `Proxy::basic_auth` stores its login in that
        // URL without escaping `%`. The login therefore goes in encoded.
        reqwest::Proxy::all(self.url_with_login().trim())
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
    reqwest::Proxy::custom(|_| Some(UNROUTABLE_PROXY_URL.to_owned()))
}

/// The address of a proxy no request gets through.
const UNROUTABLE_PROXY_URL: &str = "http://unroutable.invalid:9";

/// Where a helper process takes its proxy from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyInput {
    /// An argument the caller passes, which the program prefers over the
    /// environment, as ffmpeg and Streamlink do.
    Argument,
    /// The `HTTP_PROXY` and `HTTPS_PROXY` variables, as Go programs such as
    /// rclone and BaiduPCS-Go read them. Those accept the schemes in
    /// [`ENVIRONMENT_PROXY_SCHEMES`].
    Environment,
}

/// The proxy URL schemes Go's `net/http` accepts from the environment.
/// Go resolves host names on a `socks5` proxy, so `socks5h` is passed as
/// `socks5`, which older Go releases also read.
pub const ENVIRONMENT_PROXY_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

/// How requests reach a platform.
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

    /// Sets up the proxy environment of a helper process. A direct target
    /// removes every proxy variable, so the process connects directly
    /// whatever the server's environment says; the system target leaves the
    /// environment alone. An explicit proxy taken as an argument leaves it
    /// alone too. One taken from the environment replaces every proxy
    /// variable with `HTTP_PROXY` and `HTTPS_PROXY`, login included; callers
    /// check [`Self::supports_schemes`] first, and an address of another
    /// scheme is replaced by one no request gets through, so the process
    /// fails rather than connect without the proxy.
    pub fn apply_environment(&self, command: &mut tokio::process::Command, input: ProxyInput) {
        let explicit = match (self, input) {
            (Self::System, _) | (Self::Explicit(_), ProxyInput::Argument) => return,
            (Self::Direct, _) => None,
            (Self::Explicit(endpoint), ProxyInput::Environment) => Some(endpoint),
        };
        for name in PROXY_ENVIRONMENT {
            command.env_remove(name);
        }
        if let Some(endpoint) = explicit {
            let url = if self.supports_schemes(&ENVIRONMENT_PROXY_SCHEMES) {
                let url = endpoint.url_with_login();
                match url.strip_prefix("socks5h://") {
                    Some(rest) => format!("socks5://{rest}"),
                    None => url,
                }
            } else {
                UNROUTABLE_PROXY_URL.to_owned()
            };
            command.env("HTTP_PROXY", &url).env("HTTPS_PROXY", &url);
        }
    }
}

/// A plain-HTTP forward proxy for tests. It answers every request itself:
/// 204 when the request carries `login` (`user:password`) as its Basic proxy
/// credentials and 407 otherwise.
#[cfg(test)]
pub(crate) async fn login_checking_proxy(login: &'static str) -> std::net::SocketAddr {
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let expected = format!("Basic {}", base64::prelude::BASE64_STANDARD.encode(login));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let expected = expected.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let allowed = request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("proxy-authorization") && value.trim() == expected
                    })
                });
                let response: &[u8] = if allowed {
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                } else {
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n"
                };
                let _ = stream.write_all(response).await;
            });
        }
    });
    address
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

    /// The variables `target` sets (`Some`) and removes (`None`) for a
    /// program reading its proxy through `input`.
    fn environment(target: &ProxyTarget, input: ProxyInput) -> Vec<(String, Option<String>)> {
        let mut command = tokio::process::Command::new("program");
        target.apply_environment(&mut command, input);
        let mut envs: Vec<_> = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        envs.sort();
        envs
    }

    #[test]
    fn helper_processes_get_the_proxy_environment_their_target_and_input_call_for() {
        let explicit = |url: &str| {
            ProxyTarget::Explicit(ProxyEndpoint::new(
                url,
                Some(("us@r".into(), "p:ss".into())),
            ))
        };
        for input in [ProxyInput::Argument, ProxyInput::Environment] {
            let direct = environment(&ProxyTarget::Direct, input);
            assert_eq!(direct.len(), PROXY_ENVIRONMENT.len(), "{input:?}");
            assert!(direct.iter().all(|(_, value)| value.is_none()), "{input:?}");
            assert!(environment(&ProxyTarget::System, input).is_empty());
        }
        assert!(
            environment(&explicit("http://proxy.example:8080"), ProxyInput::Argument).is_empty()
        );

        let set = |target: &ProxyTarget| -> Vec<(String, String)> {
            let envs = environment(target, ProxyInput::Environment);
            let removed = envs.iter().filter(|(_, value)| value.is_none()).count();
            // Every other proxy variable is removed; Windows names are
            // case-insensitive, so the set ones may replace their removal.
            assert!(removed >= PROXY_ENVIRONMENT.len() - 2, "{envs:?}");
            envs.into_iter()
                .filter_map(|(name, value)| Some((name.to_uppercase(), value?)))
                .collect()
        };
        let proxied = "http://us%40r:p%3Ass@proxy.example:8080/".to_owned();
        assert_eq!(
            set(&explicit("http://proxy.example:8080")),
            [
                ("HTTPS_PROXY".to_owned(), proxied.clone()),
                ("HTTP_PROXY".to_owned(), proxied)
            ]
        );
        let socks = set(&explicit("socks5h://proxy.example:1080"));
        assert!(
            socks
                .iter()
                .all(|(_, url)| url == "socks5://us%40r:p%3Ass@proxy.example:1080"),
            "{socks:?}"
        );

        let unsupported = explicit("ftp://proxy.example:21");
        assert!(!unsupported.supports_schemes(&ENVIRONMENT_PROXY_SCHEMES));
        assert!(
            set(&unsupported)
                .iter()
                .all(|(_, url)| url == UNROUTABLE_PROXY_URL)
        );
        assert!(ProxyTarget::Direct.supports_schemes(&["http"]));
        assert!(!explicit("socks5://proxy.example:1080").supports_schemes(&["http"]));
    }

    /// A login with escapes, delimiters and spaces; the proxy must receive
    /// it as written.
    const AWKWARD_LOGIN: (&str, &str) = ("us@r", "p%41 @:x/%");

    #[tokio::test]
    async fn a_client_sends_the_login_as_written() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let (username, password) = AWKWARD_LOGIN;
            let proxy = login_checking_proxy("us@r:p%41 @:x/%").await;
            let send = |login: Option<(String, String)>| async move {
                let endpoint = ProxyEndpoint::new(format!("http://{proxy}"), login);
                crate::extractor::create_client_builder(Some(endpoint))
                    .build()
                    .unwrap()
                    .get("http://site.example/")
                    .send()
                    .await
                    .unwrap()
                    .status()
            };
            assert_eq!(
                send(Some((username.to_owned(), password.to_owned()))).await,
                reqwest::StatusCode::NO_CONTENT
            );
            assert_eq!(
                send(None).await,
                reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_unusable_proxy_fails_requests_instead_of_skipping_the_proxy() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = target.local_addr().unwrap();
            let client = crate::extractor::create_client_builder(Some(ProxyEndpoint::new(
                "http://[invalid",
                None,
            )))
            .build()
            .unwrap();
            assert!(
                client
                    .get(format!("http://{address}/"))
                    .send()
                    .await
                    .is_err()
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), target.accept())
                    .await
                    .is_err(),
                "the request reached the target directly"
            );
        })
        .await
        .unwrap();
    }
}
