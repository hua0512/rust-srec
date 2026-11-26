//! URL helpers.

use url::Url;

/// Log only the origin. Paths and even query keys can contain signed material.
/// Relative and malformed URIs have no safe origin to retain.
pub fn redact_url_for_log(input: &str) -> String {
    match Url::parse(input) {
        Ok(url) if url.has_host() => format!("{}/[redacted]", url.origin().ascii_serialization()),
        _ => "[redacted URI]".to_owned(),
    }
}

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
        // Userinfo never reaches the host.
        assert_eq!(
            extract_host("https://user:pass@cdn.example.com/live"),
            Some("cdn.example.com".to_string())
        );
    }
}
