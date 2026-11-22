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
pub fn redact_url_for_display(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw.trim()) else {
        return raw.to_owned();
    };

    let mut changed = false;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        let _ = parsed.set_username("redacted");
        let _ = parsed.set_password(None);
        changed = true;
    }

    let pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    if pairs.iter().any(|(k, _)| is_sensitive_key(k)) {
        {
            let mut serializer = parsed.query_pairs_mut();
            serializer.clear();
            for (k, v) in pairs {
                if is_sensitive_key(&k) {
                    serializer.append_pair(&k, "[redacted]");
                } else {
                    serializer.append_pair(&k, &v);
                }
            }
        }
        changed = true;
    }

    if changed {
        parsed.into()
    } else {
        raw.to_owned()
    }
}
