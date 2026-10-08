//! Rules for the address and login of a saved proxy.

pub use platforms_parser::proxy::ProxyEndpoint;

use super::ProxyError;

/// Schemes every HTTP client, download engine and danmu tunnel can use.
pub(crate) const SUPPORTED_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

const MAX_USERNAME_LEN: usize = 256;
const MAX_PASSWORD_LEN: usize = 1024;

/// Rejects anything a client would not route through the proxy: the
/// shared client builder would otherwise connect directly.
pub(crate) fn validate(endpoint: &ProxyEndpoint) -> Result<(), ProxyError> {
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
            if username.len() > MAX_USERNAME_LEN || password.len() > MAX_PASSWORD_LEN {
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
pub fn canonical(endpoint: &ProxyEndpoint) -> Result<ProxyEndpoint, ProxyError> {
    validate(endpoint)?;
    let url = url::Url::parse(endpoint.url.trim())
        .map_err(|_| ProxyError::invalid("proxy URL must be a valid URL"))?;
    Ok(ProxyEndpoint {
        url: canonical_url(&url),
        username: endpoint.username.clone(),
        password: endpoint.password.clone(),
    })
}

pub(super) fn host_text(url: &url::Url) -> String {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.to_ascii_lowercase(),
        Some(url::Host::Ipv4(address)) => address.to_string(),
        Some(url::Host::Ipv6(address)) => format!("[{address}]"),
        None => String::new(),
    }
}

pub(super) fn canonical_url(url: &url::Url) -> String {
    let host = host_text(url);
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
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
}
