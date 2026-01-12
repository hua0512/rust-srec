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

use crate::database::models::engine::{
    MesioEngineConfig, MesioGapSkipStrategy, MesioHlsVariantSelectionPolicy,
    MesioHttpVersionPreference,
};
use crate::downloader::engine::traits::DownloadConfig;

/// Build HLS configuration from rust-srec DownloadConfig using HlsProtocolBuilder.
///
/// Maps headers, cookies, and proxy settings from the download configuration
/// to the mesio HlsConfig structure using the builder pattern.
pub fn build_hls_config(
    config: &DownloadConfig,
    base_config: Option<mesio::hls::HlsConfig>,
    engine_config: &MesioEngineConfig,
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

    // Map proxy settings - explicit proxy takes precedence, then system proxy
    if let Some(ref proxy_url) = config.proxy_url {
        builder = builder.proxy(parse_proxy_url(proxy_url));
        // Note: proxy() method automatically sets use_system_proxy = false
    } else {
        // No explicit proxy - respect the use_system_proxy setting
        builder = builder.use_system_proxy(config.use_system_proxy);
    }

    let mut hls_config = builder.get_config();

    apply_hls_engine_overrides(&mut hls_config, engine_config);

    hls_config
}

fn apply_hls_engine_overrides(
    hls_config: &mut mesio::hls::HlsConfig,
    engine_config: &MesioEngineConfig,
) {
    let Some(ref cfg) = engine_config.hls else {
        return;
    };

    let ms = std::time::Duration::from_millis;

    let map_gap_strategy =
        |strategy: &MesioGapSkipStrategy| -> mesio::hls::config::GapSkipStrategy {
            match strategy {
                MesioGapSkipStrategy::WaitIndefinitely => {
                    mesio::hls::config::GapSkipStrategy::WaitIndefinitely
                }
                MesioGapSkipStrategy::SkipAfterCount { count } => {
                    mesio::hls::config::GapSkipStrategy::SkipAfterCount(*count)
                }
                MesioGapSkipStrategy::SkipAfterDuration { duration_ms } => {
                    mesio::hls::config::GapSkipStrategy::SkipAfterDuration(ms(*duration_ms))
                }
                MesioGapSkipStrategy::SkipAfterBoth { count, duration_ms } => {
                    mesio::hls::config::GapSkipStrategy::SkipAfterBoth {
                        count: *count,
                        duration: ms(*duration_ms),
                    }
                }
            }
        };

    if let Some(ref base) = cfg.base {
        if let Some(v) = base.timeout_ms {
            hls_config.base.timeout = ms(v);
        }
        if let Some(v) = base.connect_timeout_ms {
            hls_config.base.connect_timeout = ms(v);
        }
        if let Some(v) = base.read_timeout_ms {
            hls_config.base.read_timeout = ms(v);
        }
        if let Some(v) = base.write_timeout_ms {
            hls_config.base.write_timeout = ms(v);
        }
        if let Some(v) = base.follow_redirects {
            hls_config.base.follow_redirects = v;
        }
        if let Some(ref v) = base.user_agent {
            hls_config.base.user_agent = v.clone();
        }
        if let Some(ref v) = base.params {
            hls_config.base.params = v.clone();
        }
        if let Some(v) = base.danger_accept_invalid_certs {
            hls_config.base.danger_accept_invalid_certs = v;
        }
        if let Some(v) = base.force_ipv4 {
            hls_config.base.force_ipv4 = v;
        }
        if let Some(v) = base.force_ipv6 {
            hls_config.base.force_ipv6 = v;
        }
        if let Some(v) = base.http_version.as_ref() {
            hls_config.base.http_version = match v {
                MesioHttpVersionPreference::Auto => mesio::config::HttpVersionPreference::Auto,
                MesioHttpVersionPreference::Http2Only => {
                    mesio::config::HttpVersionPreference::Http2Only
                }
                MesioHttpVersionPreference::Http1Only => {
                    mesio::config::HttpVersionPreference::Http1Only
                }
            };
        }
        if let Some(v) = base.http2_keep_alive_interval_ms {
            hls_config.base.http2_keep_alive_interval = Some(ms(v));
        }
        if let Some(v) = base.pool_max_idle_per_host {
            hls_config.base.pool_max_idle_per_host = v;
        }
        if let Some(v) = base.pool_idle_timeout_ms {
            hls_config.base.pool_idle_timeout = ms(v);
        }
    }

    if let Some(ref pc) = cfg.playlist_config {
        if let Some(v) = pc.initial_playlist_fetch_timeout_ms {
            hls_config.playlist_config.initial_playlist_fetch_timeout = ms(v);
        }
        if let Some(v) = pc.live_refresh_interval_ms {
            hls_config.playlist_config.live_refresh_interval = ms(v);
        }
        if let Some(v) = pc.live_max_refresh_retries {
            hls_config.playlist_config.live_max_refresh_retries = v;
        }
        if let Some(v) = pc.live_refresh_retry_delay_ms {
            hls_config.playlist_config.live_refresh_retry_delay = ms(v);
        }
        if let Some(ref policy) = pc.variant_selection_policy {
            hls_config.playlist_config.variant_selection_policy = match policy {
                MesioHlsVariantSelectionPolicy::HighestBitrate => {
                    mesio::hls::config::HlsVariantSelectionPolicy::HighestBitrate
                }
                MesioHlsVariantSelectionPolicy::LowestBitrate => {
                    mesio::hls::config::HlsVariantSelectionPolicy::LowestBitrate
                }
                MesioHlsVariantSelectionPolicy::ClosestToBitrate { target_bitrate } => {
                    mesio::hls::config::HlsVariantSelectionPolicy::ClosestToBitrate(*target_bitrate)
                }
                MesioHlsVariantSelectionPolicy::AudioOnly => {
                    mesio::hls::config::HlsVariantSelectionPolicy::AudioOnly
                }
                MesioHlsVariantSelectionPolicy::VideoOnly => {
                    mesio::hls::config::HlsVariantSelectionPolicy::VideoOnly
                }
                MesioHlsVariantSelectionPolicy::MatchingResolution { width, height } => {
                    mesio::hls::config::HlsVariantSelectionPolicy::MatchingResolution {
                        width: *width,
                        height: *height,
                    }
                }
                MesioHlsVariantSelectionPolicy::Custom { value } => {
                    mesio::hls::config::HlsVariantSelectionPolicy::Custom(value.clone())
                }
            };
        }
        if let Some(v) = pc.adaptive_refresh_enabled {
            hls_config.playlist_config.adaptive_refresh_enabled = v;
        }
        if let Some(v) = pc.adaptive_refresh_min_interval_ms {
            hls_config.playlist_config.adaptive_refresh_min_interval = ms(v);
        }
        if let Some(v) = pc.adaptive_refresh_max_interval_ms {
            hls_config.playlist_config.adaptive_refresh_max_interval = ms(v);
        }
    }

    if let Some(ref sc) = cfg.scheduler_config {
        if let Some(v) = sc.download_concurrency {
            hls_config.scheduler_config.download_concurrency = v.max(1);
        }
        if let Some(v) = sc.processed_segment_buffer_multiplier {
            hls_config
                .scheduler_config
                .processed_segment_buffer_multiplier = v.max(1);
        }
    }

    if let Some(ref fc) = cfg.fetcher_config {
        if let Some(v) = fc.segment_download_timeout_ms {
            hls_config.fetcher_config.segment_download_timeout = ms(v);
        }
        if let Some(v) = fc.max_segment_retries {
            hls_config.fetcher_config.max_segment_retries = v;
        }
        if let Some(v) = fc.segment_retry_delay_base_ms {
            hls_config.fetcher_config.segment_retry_delay_base = ms(v);
        }
        if let Some(v) = fc.key_download_timeout_ms {
            hls_config.fetcher_config.key_download_timeout = ms(v);
        }
        if let Some(v) = fc.max_key_retries {
            hls_config.fetcher_config.max_key_retries = v;
        }
        if let Some(v) = fc.key_retry_delay_base_ms {
            hls_config.fetcher_config.key_retry_delay_base = ms(v);
        }
        if let Some(v) = fc.segment_raw_cache_ttl_ms {
            hls_config.fetcher_config.segment_raw_cache_ttl = ms(v);
        }
        if let Some(v) = fc.streaming_threshold_bytes {
            hls_config.fetcher_config.streaming_threshold_bytes = v;
        }
    }

    if let Some(pc) = cfg.processor_config.as_ref()
        && let Some(v) = pc.processed_segment_ttl_ms
    {
        hls_config.processor_config.processed_segment_ttl = ms(v);
    }

    if let Some(ref dc) = cfg.decryption_config {
        if let Some(v) = dc.key_cache_ttl_ms {
            hls_config.decryption_config.key_cache_ttl = ms(v);
        }
        if let Some(v) = dc.offload_decryption_to_cpu_pool {
            hls_config.decryption_config.offload_decryption_to_cpu_pool = v;
        }
    }

    if let Some(ref cc) = cfg.cache_config {
        if let Some(v) = cc.playlist_ttl_ms {
            hls_config.cache_config.playlist_ttl = ms(v);
        }
        if let Some(v) = cc.segment_ttl_ms {
            hls_config.cache_config.segment_ttl = ms(v);
        }
        if let Some(v) = cc.decryption_key_ttl_ms {
            hls_config.cache_config.decryption_key_ttl = ms(v);
        }
    }

    // Optional durations: use tri-state (missing => unchanged, null => clear, value => set)
    if let Some(ref oc) = cfg.output_config {
        if let Some(v) = oc.live_reorder_buffer_duration_ms {
            hls_config.output_config.live_reorder_buffer_duration = ms(v);
        }
        if let Some(v) = oc.live_reorder_buffer_max_segments {
            hls_config.output_config.live_reorder_buffer_max_segments = v.max(1);
        }
        if let Some(value) = oc.live_max_overall_stall_duration_ms {
            hls_config.output_config.live_max_overall_stall_duration = value.map(ms);
        }
        if let Some(ref v) = oc.live_gap_strategy {
            hls_config.output_config.live_gap_strategy = map_gap_strategy(v);
        }
        if let Some(ref v) = oc.vod_gap_strategy {
            hls_config.output_config.vod_gap_strategy = map_gap_strategy(v);
        }
        if let Some(value) = oc.vod_segment_timeout_ms {
            hls_config.output_config.vod_segment_timeout = value.map(ms);
        }
        if let Some(ref limits) = oc.buffer_limits {
            if let Some(v) = limits.max_segments {
                hls_config.output_config.buffer_limits.max_segments = v;
            }
            if let Some(v) = limits.max_bytes {
                hls_config.output_config.buffer_limits.max_bytes = v;
            }
        }
        if let Some(v) = oc.metrics_enabled {
            hls_config.output_config.metrics_enabled = v;
        }
    }

    if let Some(ref pc) = cfg.performance_config {
        if let Some(v) = pc.decryption_offload_enabled {
            hls_config.performance_config.decryption_offload_enabled = v;
        }
        if let Some(ref prefetch) = pc.prefetch {
            if let Some(v) = prefetch.enabled {
                hls_config.performance_config.prefetch.enabled = v;
            }
            if let Some(v) = prefetch.prefetch_count {
                hls_config.performance_config.prefetch.prefetch_count = v;
            }
            if let Some(v) = prefetch.max_buffer_before_skip {
                hls_config
                    .performance_config
                    .prefetch
                    .max_buffer_before_skip = v;
            }
        }
        if let Some(ref bp) = pc.buffer_pool {
            if let Some(v) = bp.enabled {
                hls_config.performance_config.buffer_pool.enabled = v;
            }
            if let Some(v) = bp.pool_size {
                hls_config.performance_config.buffer_pool.pool_size = v;
            }
            if let Some(v) = bp.default_capacity {
                hls_config.performance_config.buffer_pool.default_capacity = v;
            }
        }
        if let Some(ref bs) = pc.batch_scheduler {
            if let Some(v) = bs.enabled {
                hls_config.performance_config.batch_scheduler.enabled = v;
            }
            if let Some(v) = bs.batch_window_ms {
                hls_config
                    .performance_config
                    .batch_scheduler
                    .batch_window_ms = v;
            }
            if let Some(v) = bs.max_batch_size {
                hls_config.performance_config.batch_scheduler.max_batch_size = v;
            }
        }
        if let Some(v) = pc.zero_copy_enabled {
            hls_config.performance_config.zero_copy_enabled = v;
        }
        if let Some(v) = pc.metrics_enabled {
            hls_config.performance_config.metrics_enabled = v;
        }
    }

    // Back-compat/shortcuts: apply only if the structured overrides above are not set.
    if cfg
        .output_config
        .as_ref()
        .and_then(|o| o.live_gap_strategy.as_ref())
        .is_none()
        && let Some(strategy) = cfg.live_gap_strategy.as_ref()
    {
        hls_config.output_config.live_gap_strategy = map_gap_strategy(strategy);
    }
    if cfg
        .performance_config
        .as_ref()
        .and_then(|p| p.prefetch.as_ref())
        .is_none()
        && let Some(prefetch) = cfg.prefetch.as_ref()
    {
        if let Some(enabled) = prefetch.enabled {
            hls_config.performance_config.prefetch.enabled = enabled;
        }
        if let Some(prefetch_count) = prefetch.prefetch_count {
            hls_config.performance_config.prefetch.prefetch_count = prefetch_count;
        }
        if let Some(max_buffer_before_skip) = prefetch.max_buffer_before_skip {
            hls_config
                .performance_config
                .prefetch
                .max_buffer_before_skip = max_buffer_before_skip;
        }
    }
    if cfg
        .scheduler_config
        .as_ref()
        .and_then(|s| s.download_concurrency)
        .is_none()
        && let Some(download_concurrency) = cfg.download_concurrency
    {
        hls_config.scheduler_config.download_concurrency = download_concurrency.max(1);
    }
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

    // Map proxy settings - explicit proxy takes precedence, then system proxy
    if let Some(ref proxy_url) = config.proxy_url {
        let proxy = parse_proxy_url(proxy_url);
        builder = builder.with_config(|cfg| {
            cfg.base.proxy = Some(proxy);
            cfg.base.use_system_proxy = false;
        });
    } else {
        // No explicit proxy - respect the use_system_proxy setting
        builder = builder.with_config(|cfg| {
            cfg.base.use_system_proxy = config.use_system_proxy;
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
            use_system_proxy: false,
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
        let hls_config = build_hls_config(&config, None, &MesioEngineConfig::default());

        // Should have default headers from mesio
        assert!(
            hls_config
                .base
                .headers
                .contains_key(reqwest::header::ACCEPT)
        );
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

        let hls_config = build_hls_config(&config, None, &MesioEngineConfig::default());

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

        let hls_config = build_hls_config(&config, None, &MesioEngineConfig::default());

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

        let hls_config = build_hls_config(&config, None, &MesioEngineConfig::default());

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
        assert!(
            flv_config
                .base
                .headers
                .contains_key(reqwest::header::ACCEPT)
        );
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
        assert_eq!(
            hls_pipeline_config.split_segments,
            default_config.split_segments
        );
        assert_eq!(
            hls_pipeline_config.segment_limiter,
            default_config.segment_limiter
        );
    }

    #[test]
    fn test_build_flv_pipeline_config_default() {
        let config = create_test_download_config();
        let flv_pipeline_config = build_flv_pipeline_config(&config);

        // Should return default config - check individual fields
        let default_config = FlvPipelineConfig::default();
        assert_eq!(
            flv_pipeline_config.duplicate_tag_filtering,
            default_config.duplicate_tag_filtering
        );
        assert_eq!(
            flv_pipeline_config.enable_low_latency,
            default_config.enable_low_latency
        );
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

    #[test]
    fn test_build_hls_config_applies_engine_overrides() {
        let config = create_test_download_config();
        let engine_config: MesioEngineConfig = serde_json::from_str(
            r#"
            {
              "hls": {
                "download_concurrency": 3,
                "live_gap_strategy": {"type":"skip_after_count","count":3},
                "prefetch": {"enabled": false, "prefetch_count": 1, "max_buffer_before_skip": 5},

                "base": {
                  "read_timeout_ms": 1234,
                  "http_version": "http1_only",
                  "pool_max_idle_per_host": 7
                },
                "playlist_config": {
                  "live_refresh_interval_ms": 777,
                  "variant_selection_policy": { "type": "closest_to_bitrate", "target_bitrate": 9000 }
                },
                "scheduler_config": {
                  "download_concurrency": 9,
                  "processed_segment_buffer_multiplier": 2
                },
                "fetcher_config": {
                  "max_segment_retries": 42,
                  "streaming_threshold_bytes": 314159
                },
                "processor_config": {
                  "processed_segment_ttl_ms": 60000
                },
                "decryption_config": {
                  "offload_decryption_to_cpu_pool": true,
                  "key_cache_ttl_ms": 3600000
                },
                "cache_config": {
                  "playlist_ttl_ms": 1000,
                  "segment_ttl_ms": 2000,
                  "decryption_key_ttl_ms": 3000
                },
                "output_config": {
                  "live_gap_strategy": { "type":"skip_after_both","count":10,"duration_ms":1000 },
                  "vod_segment_timeout_ms": null,
                  "buffer_limits": { "max_segments": 11, "max_bytes": 123456 },
                  "metrics_enabled": false
                },
                "performance_config": {
                  "prefetch": {"enabled": true, "prefetch_count": 4, "max_buffer_before_skip": 20},
                  "batch_scheduler": {"enabled": false, "batch_window_ms": 99, "max_batch_size": 123},
                  "zero_copy_enabled": false
                }
              }
            }
            "#,
        )
        .unwrap();

        let hls_config = build_hls_config(&config, None, &engine_config);
        // Structured overrides take precedence over shortcuts.
        assert_eq!(hls_config.scheduler_config.download_concurrency, 9);
        assert_eq!(
            hls_config
                .scheduler_config
                .processed_segment_buffer_multiplier,
            2
        );

        assert_eq!(
            hls_config.base.read_timeout,
            std::time::Duration::from_millis(1234)
        );
        assert_eq!(hls_config.base.pool_max_idle_per_host, 7);
        assert!(matches!(
            hls_config.base.http_version,
            mesio::config::HttpVersionPreference::Http1Only
        ));

        assert_eq!(
            hls_config.playlist_config.live_refresh_interval,
            std::time::Duration::from_millis(777)
        );
        assert!(matches!(
            hls_config.playlist_config.variant_selection_policy,
            mesio::hls::config::HlsVariantSelectionPolicy::ClosestToBitrate(9000)
        ));

        assert_eq!(hls_config.fetcher_config.max_segment_retries, 42);
        assert_eq!(hls_config.fetcher_config.streaming_threshold_bytes, 314159);
        assert_eq!(
            hls_config.processor_config.processed_segment_ttl,
            std::time::Duration::from_millis(60000)
        );
        assert!(hls_config.decryption_config.offload_decryption_to_cpu_pool);
        assert_eq!(
            hls_config.decryption_config.key_cache_ttl,
            std::time::Duration::from_millis(3600000)
        );
        assert_eq!(
            hls_config.cache_config.playlist_ttl,
            std::time::Duration::from_millis(1000)
        );
        assert_eq!(
            hls_config.cache_config.segment_ttl,
            std::time::Duration::from_millis(2000)
        );
        assert_eq!(
            hls_config.cache_config.decryption_key_ttl,
            std::time::Duration::from_millis(3000)
        );

        assert!(matches!(
            hls_config.output_config.live_gap_strategy,
            mesio::hls::config::GapSkipStrategy::SkipAfterBoth { count: 10, duration }
                if duration == std::time::Duration::from_millis(1000)
        ));
        assert!(hls_config.output_config.vod_segment_timeout.is_none());
        assert_eq!(hls_config.output_config.buffer_limits.max_segments, 11);
        assert_eq!(hls_config.output_config.buffer_limits.max_bytes, 123456);
        assert!(!hls_config.output_config.metrics_enabled);

        assert!(hls_config.performance_config.prefetch.enabled);
        assert_eq!(hls_config.performance_config.prefetch.prefetch_count, 4);
        assert_eq!(
            hls_config
                .performance_config
                .prefetch
                .max_buffer_before_skip,
            20
        );
        assert!(!hls_config.performance_config.batch_scheduler.enabled);
        assert_eq!(
            hls_config
                .performance_config
                .batch_scheduler
                .batch_window_ms,
            99
        );
        assert_eq!(
            hls_config.performance_config.batch_scheduler.max_batch_size,
            123
        );
        assert!(!hls_config.performance_config.zero_copy_enabled);
    }
}
