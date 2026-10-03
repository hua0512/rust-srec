//! URL redaction for logs, error messages, and display fields.
//!
//! Stream URLs routinely carry credentials: signed query parameters (`sign`,
//! `token`, `wsSecret`, ...) and occasionally userinfo. Anything that leaves
//! the process as text uses these helpers; identities and requests keep the
//! real URL.

use std::fmt;

use url::Url;

/// `url` with userinfo removed, every query value masked (keys are kept, as
/// they help diagnosis and are not secret), and the fragment dropped.
pub fn redact_url(url: &Url) -> Url {
    let mut redacted = url.clone();
    // Both setters only fail for URLs that cannot carry credentials.
    if redacted.set_username("").is_err() || redacted.set_password(None).is_err() {
        return redacted;
    }
    if redacted.query().is_some() {
        let keys: Vec<String> = url.query_pairs().map(|(key, _)| key.into_owned()).collect();
        let mut pairs = redacted.query_pairs_mut();
        pairs.clear();
        for key in keys {
            pairs.append_pair(&key, "***");
        }
    }
    redacted.set_fragment(None);
    redacted
}

/// [`redact_url`] for a URL held as text. Text that does not parse as an
/// absolute URL (a relative playlist URI, a malformed URL) is masked the same
/// way textually: userinfo dropped, query values masked, fragment dropped.
pub fn redact_url_str(input: &str) -> String {
    if let Ok(url) = Url::parse(input) {
        return redact_url(&url).to_string();
    }
    let without_fragment = input.split('#').next().unwrap_or_default();
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (without_fragment, None),
    };
    let mut out = strip_userinfo(base);
    if let Some(query) = query {
        out.push('?');
        let masked: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((key, _)) => format!("{key}=***"),
                None => pair.to_string(),
            })
            .collect();
        out.push_str(&masked.join("&"));
    }
    out
}

/// Drop a `user:pass@` from the authority of `scheme://user:pass@host/...`.
fn strip_userinfo(base: &str) -> String {
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => base.to_string(),
    }
}

/// A reqwest error whose message and `url()` show the redacted URL.
pub(crate) fn redact_reqwest(error: reqwest::Error) -> reqwest::Error {
    match error.url().map(redact_url) {
        Some(url) => error.with_url(url),
        None => error,
    }
}

/// Display adapter: `%Redacted(&url)` in tracing fields and `{}` in messages.
pub struct Redacted<'a>(pub &'a Url);

impl fmt::Display for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redact_url_for_log(self.0.as_str()))
    }
}

/// Log only the origin. Paths and even query keys can contain signed material.
/// Relative and malformed URIs have no safe origin to retain.
pub fn redact_url_for_log(input: &str) -> String {
    match Url::parse(input) {
        Ok(url) if url.has_host() => format!("{}/[redacted]", url.origin().ascii_serialization()),
        _ => "[redacted URI]".to_owned(),
    }
}

/// Provider and parser diagnostics may echo response bodies or authentication
/// material. Callers retain typed categories and numeric status fields separately.
pub fn redact_diagnostic(_detail: impl fmt::Display) -> &'static str {
    "[upstream diagnostic redacted]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_health_logs_and_network_debug_views_hide_account_material() {
        #[derive(Clone, Default)]
        struct Capture(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }
        let captured = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let raw =
            "https://user:password-sentinel@cdn.example/path-sentinel?key-sentinel=value-sentinel";
        let source = crate::ContentSource::new(raw, 1);
        let mut manager = crate::SourceManager::new();
        manager.add_source(source.clone());
        let error =
            crate::DownloadError::http_status(reqwest::StatusCode::FORBIDDEN, raw, "segment");
        for _ in 0..3 {
            manager.record_failure(raw, &error, std::time::Duration::from_millis(1));
        }
        manager.set_source_active(raw, true);
        let mut config = crate::DownloaderConfig::default();
        config.headers.insert(
            "Authorization",
            reqwest::header::HeaderValue::from_static("Bearer header-sentinel"),
        );
        config
            .params
            .push(("secret".into(), "parameter-sentinel".into()));
        let key = crate::cache::CacheKey::new(
            crate::cache::CacheResourceType::Segment,
            raw,
            Some("identifier-sentinel".into()),
        );
        let rendered = format!(
            "{} {source:?} {manager:?} {config:?} {key:?}",
            String::from_utf8_lossy(&captured.0.lock())
        );
        assert!(rendered.contains("Source health updated"), "{rendered}");
        assert!(rendered.contains("temporarily disabled"), "{rendered}");
        assert!(rendered.contains("cdn.example"));
        assert!(!rendered.contains("sentinel"), "{rendered}");
    }

    #[test]
    fn log_url_and_diagnostics_never_expose_signed_paths_keys_or_body_echoes() {
        let raw = "https://user:password-sentinel@cdn.example/private-path-sentinel?query-key-sentinel=value-sentinel#fragment-sentinel";
        let rendered = Redacted(&Url::parse(raw).unwrap()).to_string();
        assert_eq!(rendered, "https://cdn.example/[redacted]");
        assert_eq!(
            redact_url_for_log("relative/path-sentinel?secret-sentinel"),
            "[redacted URI]"
        );
        assert_eq!(
            redact_diagnostic(format!("Authorization: token-sentinel {raw}")),
            "[upstream diagnostic redacted]"
        );
    }

    #[test]
    fn masks_query_values_and_credentials_but_keeps_structure() {
        let url = Url::parse(
            "https://user:pass@cdn.example.com/live/stream.flv?wsSecret=abc123&wsTime=99&sign=x#frag",
        )
        .unwrap();

        assert_eq!(
            redact_url(&url).as_str(),
            "https://cdn.example.com/live/stream.flv?wsSecret=***&wsTime=***&sign=***"
        );
    }

    #[test]
    fn urls_without_secrets_are_unchanged() {
        let url = Url::parse("https://cdn.example.com/live/index.m3u8").unwrap();
        assert_eq!(redact_url(&url), url);
    }

    #[test]
    fn relative_and_malformed_uris_are_masked_textually() {
        assert_eq!(
            redact_url_str("seg100.ts?token=abc&expires=1#x"),
            "seg100.ts?token=***&expires=***"
        );
        assert_eq!(
            redact_url_str("https://user:pw@[bad-host/seg.ts?sign=s"),
            "https://[bad-host/seg.ts?sign=***"
        );
    }

    #[tokio::test]
    async fn reqwest_errors_carry_the_redacted_url() {
        // Nothing listens on port 9 (discard) of the loopback address.
        let client = crate::downloader::create_client(&Default::default()).unwrap();
        let error = client
            .get("http://127.0.0.1:9/stream.flv?token=secret")
            .send()
            .await
            .expect_err("connection refused");

        let redacted = redact_reqwest(error);

        assert!(!redacted.to_string().contains("secret"), "{redacted}");
        assert_eq!(
            redacted.url().map(Url::as_str),
            Some("http://127.0.0.1:9/stream.flv?token=***")
        );
    }
}
