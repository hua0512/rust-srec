//! Redaction of engine process arguments before they reach the log sinks.

/// Placeholder substituted for every credential-bearing argument value.
const REDACTED: &str = "[redacted]";

/// Flags whose following argument is credential material in full.
///
/// `FfmpegEngine::build_args` folds the `Cookie` header into the CRLF-joined
/// `-headers` value, and both engines hand the proxy flags
/// `ProxyConfig::effective_url()`, which embeds `user:pass@`.
const OPAQUE_VALUE_FLAGS: &[&str] = &[
    "-headers",
    "-cookies",
    "-http_proxy",
    "--http-proxy",
    "--https-proxy",
];

/// Flags whose following argument has the form `name=value`.
///
/// `StreamlinkEngine::build_streamlink_args` emits `--http-cookie` and
/// `--http-header` this way; the name is kept so the argument stays readable.
const NAMED_VALUE_FLAGS: &[&str] = &["--http-cookie", "--http-header"];

/// How the argument following a matched flag is rewritten.
enum RedactionKind {
    /// The whole value is replaced.
    Opaque,
    /// The value is `name=value`; only the part after the first `=` is replaced.
    Named,
}

/// Copy of `args` with cookie, header and proxy-credential values replaced by
/// `[redacted]`, for logging the command line of a spawned engine process.
///
/// Only the value following a known flag is rewritten, so flag names, the stream
/// URL and the output path stay intact.
pub fn redact_process_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next: Option<RedactionKind> = None;

    for arg in args {
        match redact_next.take() {
            Some(RedactionKind::Opaque) => redacted.push(REDACTED.to_string()),
            Some(RedactionKind::Named) => redacted.push(match arg.split_once('=') {
                Some((name, _)) => format!("{name}={REDACTED}"),
                None => REDACTED.to_string(),
            }),
            None => {
                let flag = arg.as_str();
                if OPAQUE_VALUE_FLAGS.contains(&flag) {
                    redact_next = Some(RedactionKind::Opaque);
                } else if NAMED_VALUE_FLAGS.contains(&flag) {
                    redact_next = Some(RedactionKind::Named);
                }
                redacted.push(arg.clone());
            }
        }
    }

    redacted
}

#[cfg(test)]
mod tests {
    use super::{REDACTED, redact_process_args};

    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn redacts_ffmpeg_header_block_and_proxy_url() {
        let rendered = format!(
            "{:?}",
            redact_process_args(&args(&[
                "-y",
                "-http_proxy",
                "http://proxy-user:proxy-sentinel@proxy.example:8080",
                "-headers",
                "Referer: https://example.com\r\nCookie: SESSDATA=cookie-sentinel",
                "-i",
                "https://example.com/live.flv",
                "/recordings/out.flv",
            ]))
        );

        assert!(!rendered.contains("cookie-sentinel"), "{rendered}");
        assert!(!rendered.contains("proxy-sentinel"), "{rendered}");
        // Flags and non-secret arguments stay intact for diagnostics.
        assert!(rendered.contains("-http_proxy"), "{rendered}");
        assert!(rendered.contains("-headers"), "{rendered}");
        assert!(
            rendered.contains("https://example.com/live.flv"),
            "{rendered}"
        );
        assert!(rendered.contains("/recordings/out.flv"), "{rendered}");
        assert_eq!(rendered.matches(REDACTED).count(), 2, "{rendered}");
    }

    #[test]
    fn redacts_streamlink_cookie_and_header_values_but_keeps_names() {
        let rendered = format!(
            "{:?}",
            redact_process_args(&args(&[
                "--stdout",
                "--http-proxy",
                "http://user:proxy-sentinel@proxy.example:8080",
                "--http-cookie",
                "SESSDATA=cookie-sentinel",
                "--http-header",
                "Authorization=Bearer token-sentinel",
                "https://example.com/live",
                "best",
            ]))
        );

        for secret in ["cookie-sentinel", "proxy-sentinel", "token-sentinel"] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
        assert!(rendered.contains("SESSDATA="), "{rendered}");
        assert!(rendered.contains("Authorization="), "{rendered}");
        assert!(rendered.contains("best"), "{rendered}");
    }

    #[test]
    fn redacts_a_named_value_flag_without_an_equals_sign() {
        let rendered = format!(
            "{:?}",
            redact_process_args(&args(&["--http-cookie", "raw-cookie-sentinel"]))
        );

        assert!(!rendered.contains("raw-cookie-sentinel"), "{rendered}");
    }

    #[test]
    fn leaves_arguments_without_credential_flags_untouched() {
        let original = args(&["-y", "-c", "copy", "-i", "pipe:0", "out.mp4"]);
        assert_eq!(redact_process_args(&original), original);
    }

    #[test]
    fn a_trailing_flag_without_a_value_does_not_panic() {
        assert_eq!(
            redact_process_args(&args(&["-i", "url", "-headers"])),
            args(&["-i", "url", "-headers"])
        );
    }
}
