//! URL helpers.

/// Extracts the `host[:port]` part from an absolute http(s) URL.
///
/// Returns `None` when the URL is not absolute http(s) or has no host.
pub fn extract_host(url: &str) -> Option<String> {
    // Accept only absolute http(s) URLs.
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;

    // host[:port] is until first '/', '?', or '#'.
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut host_port = &rest[..end];

    // Strip potential userinfo (user:pass@host).
    if let Some(at) = host_port.rfind('@') {
        host_port = &host_port[at + 1..];
    }

    if host_port.is_empty() {
        None
    } else {
        Some(host_port.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_host_and_port() {
        assert_eq!(
            extract_host("https://cdn.example.com:8443/path?x=1"),
            Some("cdn.example.com:8443".to_string())
        );
    }

    #[test]
    fn extracts_host_without_path() {
        assert_eq!(
            extract_host("http://cdn.example.com"),
            Some("cdn.example.com".to_string())
        );
    }

    #[test]
    fn strips_userinfo() {
        assert_eq!(
            extract_host("https://user:pass@cdn.example.com/live"),
            Some("cdn.example.com".to_string())
        );
    }

    #[test]
    fn rejects_non_http() {
        assert_eq!(extract_host("rtmp://example.com/live"), None);
        assert_eq!(extract_host("/relative/path"), None);
    }
}
