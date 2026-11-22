pub mod formats;
pub mod media_info;
pub mod platform_metadata;
pub mod stream_info;

pub use formats::{MediaFormat, StreamFormat};
pub use media_info::MediaInfo;
pub use platform_metadata::PlatformMetadata;
pub use stream_info::StreamInfo;

/// Whether a header, extra, or query parameter key carries credentials,
/// cookies, passwords, or signing secrets that must be redacted in human-readable
/// display output.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-s"
            | "pwd"
            | "ftk"
            | "ttwid"
            | "sign"
            | "sig"
            | "signature"
            | "auth"
            | "auth_key"
            | "hdnea"
            | "policy"
            | "key-pair-id"
    ) || lower.contains("cookie")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("token")
}

/// Redacts embedded credentials and sensitive query parameters from `url` for
/// human-readable summaries while preserving non-sensitive path and query context.
/// A link written without a scheme is redacted the same way and shown bare.
pub fn redact_url_for_display(raw: &str) -> String {
    const BARE_LINK_SCHEME: &str = "https://";

    let trimmed = raw.trim();
    let redacted = match url::Url::parse(trimmed) {
        Ok(parsed) if parsed.has_host() => redact_parsed_url(parsed),
        // Without a scheme the link is read as the extractors read it. This
        // also covers `user:pass@host`, which parses as a URL of scheme `user`.
        _ => url::Url::parse(&format!("{BARE_LINK_SCHEME}{trimmed}"))
            .ok()
            .and_then(redact_parsed_url)
            .map(|redacted| match redacted.strip_prefix(BARE_LINK_SCHEME) {
                Some(bare) => bare.to_owned(),
                None => redacted,
            }),
    };
    redacted.unwrap_or_else(|| raw.to_owned())
}

/// The redacted form of `parsed`, or `None` when it holds nothing sensitive.
fn redact_parsed_url(mut parsed: url::Url) -> Option<String> {
    const PLACEHOLDER: &str = "[redacted]";

    let mut changed = false;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        if parsed.set_username("redacted").is_err() || parsed.set_password(None).is_err() {
            // The login could not be replaced, so no part of the URL is shown.
            return Some(PLACEHOLDER.to_owned());
        }
        changed = true;
    }

    // Segments are rewritten in place rather than re-serialised: form encoding
    // would escape the placeholder's brackets and re-encode the other values.
    if let Some(query) = parsed.query() {
        let mut redacted_any = false;
        let segments: Vec<String> = query
            .split('&')
            .map(|segment| {
                let raw_key = segment.split_once('=').map_or(segment, |(key, _)| key);
                let key = url::form_urlencoded::parse(raw_key.as_bytes())
                    .next()
                    .map(|(key, _)| key);
                if key.is_some_and(|key| is_sensitive_key(&key)) {
                    redacted_any = true;
                    format!("{raw_key}={PLACEHOLDER}")
                } else {
                    segment.to_owned()
                }
            })
            .collect();
        if redacted_any {
            parsed.set_query(Some(&segments.join("&")));
            changed = true;
        }
    }

    changed.then(|| parsed.into())
}
