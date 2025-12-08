//! Configuration mapping utilities for mesio download engine.
//!
//! This module provides functions to map rust-srec's `DownloadConfig` to
//! the configuration structures used by the mesio crate for HLS and FLV
//! protocol handling.

use flv_fix::FlvPipelineConfig;
use hls_fix::HlsPipelineConfig;
use mesio::flv::FlvProtocolConfig;
use mesio::proxy::{ProxyConfig, ProxyType};
use mesio::{FlvProtocolBuilder, HlsProtocolBuilder};
use pipeline_common::config::PipelineConfig;
use tracing::debug;

use crate::downloader::engine::traits::DownloadConfig;

/// Build HLS configuration from rust-srec DownloadConfig using HlsProtocolBuilder.
///
/// Maps headers, cookies, and proxy settings from the download configuration
/// to the mesio HlsConfig structure using the builder pattern.
pub fn build_hls_config(
    config: &DownloadConfig,
    base_config: Option<mesio::hls::HlsConfig>,
) -> mesio::hls::HlsConfig {
    let mut builder = if let Some(base) = base_config {
        HlsProtocolBuilder::new().with_config(|c| *c = base)
    } else {
        HlsProtocolBuilder::new()
    };

    // Map headers
    for (key, value) in &config.headers {
        debug!("Adding header: {} = {}", key, value);
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // Map proxy settings
    if let Some(ref proxy_url) = config.proxy_url {
        builder = builder.proxy(parse_proxy_url(proxy_url));
    }

    builder.get_config()
}

/// Build FLV configuration from rust-srec DownloadConfig using FlvProtocolBuilder.
///
/// Maps headers, cookies, and proxy settings from the download configuration
/// to the mesio FlvProtocolConfig structure using the builder pattern.
pub fn build_flv_config(
    config: &DownloadConfig,
    base_config: Option<FlvProtocolConfig>,
) -> FlvProtocolConfig {
    let mut builder = if let Some(base) = base_config {
        FlvProtocolBuilder::new().with_config(|c| *c = base)
    } else {
        FlvProtocolBuilder::new()
    };

    // Map headers
    for (key, value) in &config.headers {
        debug!("Adding header : {}={}", key, value);
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // Map proxy settings using with_config since FlvProtocolBuilder doesn't have a proxy method
    if let Some(ref proxy_url) = config.proxy_url {
        let proxy = parse_proxy_url(proxy_url);
        builder = builder.with_config(|cfg| {
            cfg.base.proxy = Some(proxy);
        });
    }

    builder.get_config()
}

/// Build PipelineConfig from rust-srec DownloadConfig.
///
/// Maps max_file_size, max_duration, and channel_size settings from the download
/// configuration to the pipeline-common PipelineConfig structure.
///
/// If `pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, builds a new PipelineConfig from the individual settings.
pub fn build_pipeline_config(config: &DownloadConfig) -> PipelineConfig {
    if let Some(ref pipeline_config) = config.pipeline_config {
        pipeline_config.clone()
    } else {
        let mut builder = PipelineConfig::builder()
            .max_file_size(config.max_segment_size_bytes)
            .channel_size(64);

        if config.max_segment_duration_secs > 0 {
            builder = builder.max_duration(std::time::Duration::from_secs(
                config.max_segment_duration_secs,
            ));
        }

        builder.build()
    }
}

/// Build HlsPipelineConfig from rust-srec DownloadConfig.
///
/// If `hls_pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, returns the default HlsPipelineConfig.
pub fn build_hls_pipeline_config(config: &DownloadConfig) -> HlsPipelineConfig {
    config.hls_pipeline_config.clone().unwrap_or_default()
}

/// Build FlvPipelineConfig from rust-srec DownloadConfig.
///
/// If `flv_pipeline_config` is already set on the DownloadConfig, returns a clone of it.
/// Otherwise, returns the default FlvPipelineConfig.
pub fn build_flv_pipeline_config(config: &DownloadConfig) -> FlvPipelineConfig {
    config.flv_pipeline_config.clone().unwrap_or_default()
}

/// Parse a proxy URL string into a ProxyConfig.
///
/// Supports HTTP, HTTPS, and SOCKS5 proxy URLs.
/// Format: `[protocol://][user:pass@]host:port`
fn parse_proxy_url(url: &str) -> ProxyConfig {
    let url_lower = url.to_lowercase();

    // Determine proxy type from URL scheme
    let proxy_type = if url_lower.starts_with("socks5://") || url_lower.starts_with("socks5h://") {
        ProxyType::Socks5
    } else if url_lower.starts_with("https://") {
        ProxyType::Https
    } else {
        // Default to HTTP for http:// or no scheme
        ProxyType::Http
    };

    // Extract authentication if present (user:pass@host format)
    let auth = extract_proxy_auth(url);

    ProxyConfig {
        url: url.to_string(),
        proxy_type,
        auth,
    }
}

/// Extract authentication credentials from a proxy URL if present.
///
/// Looks for the pattern `user:pass@` in the URL.
fn extract_proxy_auth(url: &str) -> Option<mesio::proxy::ProxyAuth> {
    // Find the scheme separator
    let url_without_scheme = if let Some(pos) = url.find("://") {
        &url[pos + 3..]
    } else {
        url
    };

    // Check for @ which indicates auth credentials
    if let Some(at_pos) = url_without_scheme.find('@') {
        let auth_part = &url_without_scheme[..at_pos];
        if let Some(colon_pos) = auth_part.find(':') {
            let username = auth_part[..colon_pos].to_string();
            let password = auth_part[colon_pos + 1..].to_string();
            return Some(mesio::proxy::ProxyAuth { username, password });
        }
    }

    None
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_test_download_config() -> DownloadConfig {
        DownloadConfig {
            url: "https://example.com/stream.m3u8".to_string(),
            output_dir: PathBuf::from("/tmp/downloads"),
            filename_template: "test-stream".to_string(),
            output_format: "ts".to_string(),
            max_segment_duration_secs: 0,
            max_segment_size_bytes: 0,
            proxy_url: None,
            cookies: None,
            headers: Vec::new(),
            streamer_id: "test-streamer".to_string(),
            session_id: "test-session".to_string(),
            enable_processing: false,
            pipeline_config: None,
            hls_pipeline_config: None,
            flv_pipeline_config: None,
            engines_override: None,
        }
    }

    #[test]
    fn test_build_hls_config_default() {
        let config = create_test_download_config();
        let hls_config = build_hls_config(&config, None);

        // Should have default headers from mesio
        assert!(hls_config.base.headers.contains_key(reqwest::header::ACCEPT));
        // Should not have proxy configured
        assert!(hls_config.base.proxy.is_none());
    }

    #[test]
    fn test_build_hls_config_with_headers() {
        let mut config = create_test_download_config();
        config.headers = vec![
            ("User-Agent".to_string(), "CustomAgent/1.0".to_string()),
            ("X-Custom-Header".to_string(), "custom-value".to_string()),
        ];

        let hls_config = build_hls_config(&config, None);

        // Check custom headers are mapped
        assert_eq!(
            hls_config
                .base
                .headers
                .get(reqwest::header::USER_AGENT)
                .map(|v| v.to_str().unwrap()),
            Some("CustomAgent/1.0")
        );
        assert_eq!(
            hls_config
                .base
                .headers
                .get("X-Custom-Header")
                .map(|v| v.to_str().unwrap()),
            Some("custom-value")
        );
    }

    #[test]
    fn test_build_hls_config_with_cookies() {
        let mut config = create_test_download_config();
        config.cookies = Some("session=abc123; token=xyz789".to_string());

        let hls_config = build_hls_config(&config, None);

        // Check cookies are mapped to Cookie header
        assert_eq!(
            hls_config
                .base
                .headers
                .get(reqwest::header::COOKIE)
                .map(|v| v.to_str().unwrap()),
            Some("session=abc123; token=xyz789")
        );
    }

    #[test]
    fn test_build_hls_config_with_proxy() {
        let mut config = create_test_download_config();
        config.proxy_url = Some("http://proxy.example.com:8080".to_string());

        let hls_config = build_hls_config(&config, None);

        // Check proxy is configured
        assert!(hls_config.base.proxy.is_some());
        let proxy = hls_config.base.proxy.unwrap();
        assert_eq!(proxy.url, "http://proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
    }

    #[test]
    fn test_build_flv_config_default() {
        let config = create_test_download_config();
        let flv_config = build_flv_config(&config, None);

        // Should have default headers from mesio
        assert!(flv_config.base.headers.contains_key(reqwest::header::ACCEPT));
        // Should not have proxy configured
        assert!(flv_config.base.proxy.is_none());
    }

    #[test]
    fn test_build_flv_config_with_headers() {
        let mut config = create_test_download_config();
        config.headers = vec![("Referer".to_string(), "https://example.com".to_string())];

        let flv_config = build_flv_config(&config, None);

        // Check custom headers are mapped
        assert_eq!(
            flv_config
                .base
                .headers
                .get(reqwest::header::REFERER)
                .map(|v| v.to_str().unwrap()),
            Some("https://example.com")
        );
    }

    #[test]
    fn test_build_flv_config_with_cookies() {
        let mut config = create_test_download_config();
        config.cookies = Some("auth=secret".to_string());

        let flv_config = build_flv_config(&config, None);

        // Check cookies are mapped to Cookie header
        assert_eq!(
            flv_config
                .base
                .headers
                .get(reqwest::header::COOKIE)
                .map(|v| v.to_str().unwrap()),
            Some("auth=secret")
        );
    }

    #[test]
    fn test_parse_proxy_url_http() {
        let proxy = parse_proxy_url("http://proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
        assert_eq!(proxy.url, "http://proxy.example.com:8080");
        assert!(proxy.auth.is_none());
    }

    #[test]
    fn test_parse_proxy_url_https() {
        let proxy = parse_proxy_url("https://secure-proxy.example.com:443");
        assert_eq!(proxy.proxy_type, ProxyType::Https);
        assert_eq!(proxy.url, "https://secure-proxy.example.com:443");
    }

    #[test]
    fn test_parse_proxy_url_socks5() {
        let proxy = parse_proxy_url("socks5://socks-proxy.example.com:1080");
        assert_eq!(proxy.proxy_type, ProxyType::Socks5);
        assert_eq!(proxy.url, "socks5://socks-proxy.example.com:1080");
    }

    #[test]
    fn test_parse_proxy_url_with_auth() {
        let proxy = parse_proxy_url("http://user:password@proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
        assert!(proxy.auth.is_some());
        let auth = proxy.auth.unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "password");
    }

    #[test]
    fn test_parse_proxy_url_no_scheme() {
        // URLs without scheme should default to HTTP
        let proxy = parse_proxy_url("proxy.example.com:8080");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
    }

    #[test]
    fn test_extract_proxy_auth_with_credentials() {
        let auth = extract_proxy_auth("http://user:pass@host:8080");
        assert!(auth.is_some());
        let auth = auth.unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "pass");
    }

    #[test]
    fn test_extract_proxy_auth_without_credentials() {
        let auth = extract_proxy_auth("http://host:8080");
        assert!(auth.is_none());
    }

    #[test]
    fn test_build_pipeline_config_default() {
        let config = create_test_download_config();
        let pipeline_config = build_pipeline_config(&config);

        assert_eq!(pipeline_config.channel_size, 64);
        assert_eq!(pipeline_config.max_file_size, 0);
    }

    #[test]
    fn test_build_pipeline_config_with_max_duration() {
        let mut config = create_test_download_config();
        config.max_segment_duration_secs = 3600;

        let pipeline_config = build_pipeline_config(&config);

        assert_eq!(
            pipeline_config.max_duration,
            Some(std::time::Duration::from_secs(3600))
        );
    }

    #[test]
    fn test_build_hls_pipeline_config_default() {
        let config = create_test_download_config();
        let hls_pipeline_config = build_hls_pipeline_config(&config);

        // Should return default config - check individual fields
        let default_config = HlsPipelineConfig::default();
        assert_eq!(hls_pipeline_config.defragment, default_config.defragment);
        assert_eq!(hls_pipeline_config.split_segments, default_config.split_segments);
        assert_eq!(hls_pipeline_config.segment_limiter, default_config.segment_limiter);
    }

    #[test]
    fn test_build_flv_pipeline_config_default() {
        let config = create_test_download_config();
        let flv_pipeline_config = build_flv_pipeline_config(&config);

        // Should return default config - check individual fields
        let default_config = FlvPipelineConfig::default();
        assert_eq!(flv_pipeline_config.duplicate_tag_filtering, default_config.duplicate_tag_filtering);
        assert_eq!(flv_pipeline_config.enable_low_latency, default_config.enable_low_latency);
        assert_eq!(flv_pipeline_config.pipe_mode, default_config.pipe_mode);
    }

    #[test]
    fn test_build_pipeline_config_with_max_size() {
        let mut config = create_test_download_config();
        config.max_segment_size_bytes = 1024 * 1024 * 100; // 100 MB

        let pipeline_config = build_pipeline_config(&config);

        assert_eq!(pipeline_config.max_file_size, 1024 * 1024 * 100);
    }

    #[test]
    fn test_build_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        // Set explicit pipeline config
        config.pipeline_config = Some(
            PipelineConfig::builder()
                .max_file_size(500_000_000)
                .max_duration(std::time::Duration::from_secs(7200))
                .channel_size(128)
                .build(),
        );

        let pipeline_config = build_pipeline_config(&config);

        // Should use the explicit config, not build from individual fields
        assert_eq!(pipeline_config.max_file_size, 500_000_000);
        assert_eq!(
            pipeline_config.max_duration.unwrap(),
            std::time::Duration::from_secs(7200)
        );
        assert_eq!(pipeline_config.channel_size, 128);
    }

    #[test]
    fn test_build_hls_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        config.hls_pipeline_config = Some(HlsPipelineConfig {
            defragment: false,
            split_segments: true,
            segment_limiter: false,
        });

        let hls_pipeline_config = build_hls_pipeline_config(&config);

        assert!(!hls_pipeline_config.defragment);
        assert!(hls_pipeline_config.split_segments);
        assert!(!hls_pipeline_config.segment_limiter);
    }

    #[test]
    fn test_build_flv_pipeline_config_with_explicit_config() {
        let mut config = create_test_download_config();
        config.flv_pipeline_config = Some(
            FlvPipelineConfig::builder()
                .duplicate_tag_filtering(false)
                .enable_low_latency(false)
                .pipe_mode(true)
                .build(),
        );

        let flv_pipeline_config = build_flv_pipeline_config(&config);

        assert!(!flv_pipeline_config.duplicate_tag_filtering);
        assert!(!flv_pipeline_config.enable_low_latency);
        assert!(flv_pipeline_config.pipe_mode);
    }
}
