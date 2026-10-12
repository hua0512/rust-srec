//! Configuration mapping utilities for mesio download engine.
//!
//! This module provides functions to map rust-srec's `DownloadConfig` to
//! the configuration structures used by the mesio crate for HLS, DASH and
//! FLV protocol handling.

use std::time::Duration;

use mesio::dash::{AudioSelection, LiveStartPolicy, VideoSelection};
use mesio::flv::FlvProtocolConfig;
use mesio::proxy::{ProxyAuth, ProxyConfig, ProxyType};
use mesio::{
    DashConfig, DashProtocolBuilder, DownloadError, FlvProtocolBuilder, HlsProtocolBuilder,
};
use tracing::debug;

use crate::database::models::engine::{
    MesioDashAudioQuality, MesioDashConfig, MesioDashLiveStart, MesioDashVideoQuality,
    MesioEngineConfig, MesioGapSkipStrategy, MesioHlsVariantSelectionPolicy,
    MesioHttpVersionPreference,
};
use crate::downloader::engine::traits::DownloadConfig;
use crate::proxies::{ProxyEndpoint, ProxyTarget};

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
        debug!("Adding header: {}", key);
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // A direct route turns off the environment's proxy too.
    builder = match &config.proxy {
        ProxyTarget::Explicit(endpoint) => builder.proxy(mesio_proxy(endpoint)),
        ProxyTarget::System => builder.use_system_proxy(true),
        ProxyTarget::Direct => builder.use_system_proxy(false),
    };

    let builder = apply_hls_engine_overrides(builder, engine_config);

    builder.get_config()
}

fn apply_hls_engine_overrides(
    mut builder: HlsProtocolBuilder,
    engine_config: &MesioEngineConfig,
) -> HlsProtocolBuilder {
    let Some(ref cfg) = engine_config.hls else {
        return builder;
    };

    let ms = std::time::Duration::from_millis;

    if let Some(ref base) = cfg.base {
        if let Some(v) = base.timeout_ms {
            builder = builder.timeout(ms(v));
        }
        if let Some(v) = base.connect_timeout_ms {
            builder = builder.connect_timeout(ms(v));
        }
        if let Some(v) = base.read_timeout_ms {
            builder = builder.read_timeout(ms(v));
        }
        if let Some(v) = base.write_timeout_ms {
            builder = builder.write_timeout(ms(v));
        }
        if let Some(v) = base.follow_redirects {
            builder = builder.follow_redirects(v);
        }
        if let Some(ref v) = base.user_agent {
            builder = builder.user_agent(v.clone());
        }
        if let Some(v) = base.danger_accept_invalid_certs {
            builder = builder.danger_accept_invalid_certs(v);
        }

        builder = builder.with_config(|hls_config| {
            if let Some(ref v) = base.params {
                hls_config.base.params = v.clone();
            }
            if let Some(v) = base.force_ipv4 {
                hls_config.base.force_ipv4 = v;
            }
            if let Some(v) = base.force_ipv6 {
                hls_config.base.force_ipv6 = v;
            }
            if let Some(v) = base.http_version.as_ref() {
                hls_config.base.http_version = map_http_version(v);
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
        });
    }

    if let Some(ref pc) = cfg.playlist_config {
        if let Some(v) = pc.initial_playlist_fetch_timeout_ms {
            builder = builder.initial_playlist_fetch_timeout(ms(v));
        }
        if let Some(v) = pc.live_refresh_interval_ms {
            builder = builder.live_refresh_interval(ms(v));
        }
        if let Some(v) = pc.live_max_refresh_retries {
            builder = builder.live_max_refresh_retries(v);
        }
        if let Some(v) = pc.live_refresh_retry_delay_ms {
            builder = builder.live_refresh_retry_delay(ms(v));
        }
        if let Some(ref policy) = pc.variant_selection_policy {
            builder = builder.variant_selection_policy(map_variant_selection_policy(policy));
        }

        builder = builder.with_config(|hls_config| {
            if let Some(v) = pc.adaptive_refresh_enabled {
                hls_config.playlist_config.adaptive_refresh_enabled = v;
            }
            if let Some(v) = pc.adaptive_refresh_min_interval_ms {
                hls_config.playlist_config.adaptive_refresh_min_interval = ms(v);
            }
            if let Some(v) = pc.adaptive_refresh_max_interval_ms {
                hls_config.playlist_config.adaptive_refresh_max_interval = ms(v);
            }
        });
    }

    if let Some(ref sc) = cfg.scheduler_config {
        if let Some(v) = sc.download_concurrency {
            builder = builder.download_concurrency(v.max(1));
        }

        builder = builder.with_config(|hls_config| {
            if let Some(v) = sc.processed_segment_buffer_multiplier {
                hls_config
                    .scheduler_config
                    .processed_segment_buffer_multiplier = v.max(1);
            }
        });
    }

    if let Some(ref fc) = cfg.fetcher_config {
        if let Some(v) = fc.segment_download_timeout_ms {
            builder = builder.segment_download_timeout(ms(v));
        }
        if let Some(v) = fc.max_segment_retries {
            builder = builder.max_segment_retries(v);
        }
        if let Some(v) = fc.segment_retry_delay_base_ms {
            builder = builder.segment_retry_delay_base(ms(v));
        }
        if let Some(v) = fc.key_download_timeout_ms {
            builder = builder.key_download_timeout(ms(v));
        }
        if let Some(v) = fc.max_key_retries {
            builder = builder.max_key_retries(v);
        }
        if let Some(v) = fc.key_retry_delay_base_ms {
            builder = builder.key_retry_delay_base(ms(v));
        }
        builder = builder.with_config(|hls_config| {
            if let Some(v) = fc.max_segment_retry_delay_ms {
                hls_config.fetcher_config.max_segment_retry_delay = ms(v);
            }
            if let Some(v) = fc.max_key_retry_delay_ms {
                hls_config.fetcher_config.max_key_retry_delay = ms(v);
            }
            if let Some(v) = fc.progress_emit_min_bytes {
                hls_config.fetcher_config.progress_emit_min_bytes = v;
            }
            if let Some(v) = fc.progress_emit_min_interval_ms {
                hls_config.fetcher_config.progress_emit_min_interval = ms(v);
            }
        });
    }

    if let Some(pc) = cfg.processor_config.as_ref()
        && let Some(v) = pc.processed_segment_ttl_ms
    {
        builder = builder.processed_segment_ttl(ms(v));
    }

    if let Some(ref dc) = cfg.decryption_config {
        if let Some(v) = dc.key_cache_ttl_ms {
            builder = builder.decryption_key_cache_ttl(ms(v));
        }
        if let Some(v) = dc.offload_decryption_to_cpu_pool {
            builder = builder.offload_decryption_to_cpu_pool(v);
        }
    }

    if let Some(ref cc) = cfg.cache_config {
        if let Some(v) = cc.playlist_ttl_ms {
            builder = builder.hls_playlist_cache_ttl(ms(v));
        }
        if let Some(v) = cc.segment_ttl_ms {
            builder = builder.hls_segment_cache_ttl(ms(v));
        }
        if let Some(v) = cc.decryption_key_ttl_ms {
            builder = builder.hls_decryption_key_cache_ttl(ms(v));
        }
    }

    // Optional durations: use tri-state (missing => unchanged, null => clear, value => set)
    if let Some(ref oc) = cfg.output_config {
        if let Some(v) = oc.live_reorder_buffer_duration_ms {
            builder = builder.live_reorder_buffer_duration(ms(v));
        }
        if let Some(v) = oc.live_reorder_buffer_max_segments {
            builder = builder.live_reorder_buffer_max_segments(v.max(1));
        }
        if let Some(value) = oc.live_max_overall_stall_duration_ms {
            builder = builder.live_max_overall_stall_duration(value.map(ms));
        }

        builder = builder.with_config(|hls_config| {
            if let Some(v) = oc.gap_evaluation_interval_ms {
                hls_config.output_config.gap_evaluation_interval = ms(v.max(1));
            }
            if let Some(v) = oc.max_pending_init_segments {
                hls_config.output_config.max_pending_init_segments = v;
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
        });
    }

    builder
}

fn map_gap_strategy(strategy: &MesioGapSkipStrategy) -> mesio::hls::config::GapSkipStrategy {
    let ms = std::time::Duration::from_millis;

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
}

fn map_variant_selection_policy(
    policy: &MesioHlsVariantSelectionPolicy,
) -> mesio::hls::config::HlsVariantSelectionPolicy {
    match policy {
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
    }
}

fn map_http_version(version: &MesioHttpVersionPreference) -> mesio::config::HttpVersionPreference {
    match version {
        MesioHttpVersionPreference::Auto => mesio::config::HttpVersionPreference::Auto,
        MesioHttpVersionPreference::Http2Only => mesio::config::HttpVersionPreference::Http2Only,
        MesioHttpVersionPreference::Http1Only => mesio::config::HttpVersionPreference::Http1Only,
    }
}

/// Build DASH configuration from rust-srec DownloadConfig using
/// DashProtocolBuilder.
///
/// Maps headers, cookies and proxy settings like [`build_hls_config`], applies
/// `MesioEngineConfig.dash`, and rejects settings the engine cannot run with
/// as [`DownloadError::Configuration`].
pub fn build_dash_config(
    config: &DownloadConfig,
    base_config: Option<DashConfig>,
    engine_config: &MesioEngineConfig,
) -> Result<DashConfig, DownloadError> {
    let mut builder = if let Some(base) = base_config {
        DashProtocolBuilder::new().with_config(|c| *c = base)
    } else {
        DashProtocolBuilder::new()
    };

    for (key, value) in &config.headers {
        debug!("Adding header: {}", key);
        builder = builder.add_header(key, value);
    }

    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // A direct route turns off the environment's proxy too.
    builder = match &config.proxy {
        ProxyTarget::Explicit(endpoint) => builder.proxy(mesio_proxy(endpoint)),
        ProxyTarget::System => builder.use_system_proxy(true),
        ProxyTarget::Direct => builder.use_system_proxy(false),
    };

    if let Some(ref dash) = engine_config.dash {
        builder = apply_dash_engine_overrides(builder, dash)?;
    }

    let dash_config = builder.get_config();
    dash_config.validate()?;
    Ok(dash_config)
}

fn apply_dash_engine_overrides(
    builder: DashProtocolBuilder,
    cfg: &MesioDashConfig,
) -> Result<DashProtocolBuilder, DownloadError> {
    let invalid = |reason: &str| DownloadError::Configuration {
        reason: format!("DASH: {reason}"),
    };
    if cfg.download_concurrency == 0 {
        return Err(invalid("download_concurrency must be at least 1"));
    }
    for (name, value) in [
        ("manifest_timeout_ms", cfg.manifest_timeout_ms),
        ("min_refresh_interval_ms", cfg.min_refresh_interval_ms),
        ("segment_timeout_ms", cfg.segment_timeout_ms),
    ] {
        if value == 0 {
            return Err(invalid(&format!("{name} must be positive")));
        }
    }
    match cfg.video_quality {
        MesioDashVideoQuality::MaxHeight { height: 0 } => {
            return Err(invalid("video_quality height must be positive"));
        }
        MesioDashVideoQuality::ClosestBandwidth { bandwidth: 0 } => {
            return Err(invalid("video_quality bandwidth must be positive"));
        }
        _ => {}
    }
    if let MesioDashAudioQuality::ClosestBandwidth { bandwidth: 0 } = cfg.audio_quality {
        return Err(invalid("audio_quality bandwidth must be positive"));
    }

    let ms = Duration::from_millis;
    let mut builder = builder
        .video_selection(map_dash_video_quality(cfg.video_quality))
        .audio_selection(map_dash_audio_quality(cfg.audio_quality))
        .include_video(cfg.include_video)
        .include_audio(cfg.include_audio)
        .download_concurrency(cfg.download_concurrency)
        .manifest_fetch_timeout(ms(cfg.manifest_timeout_ms))
        .min_refresh_interval(ms(cfg.min_refresh_interval_ms))
        .max_refresh_retries(cfg.max_refresh_retries)
        .segment_download_timeout(ms(cfg.segment_timeout_ms))
        .max_segment_retries(cfg.max_segment_retries)
        .live_start(map_dash_live_start(cfg.live_start));
    if let Some(language) = cfg.audio_language.as_deref() {
        let language = language.trim();
        if !is_language_tag(language) {
            return Err(invalid(
                "audio_language must be a language tag such as 'en' or 'pt-BR'",
            ));
        }
        builder = builder.audio_language(language);
    }
    Ok(builder)
}

/// Language tag shape: subtags of 1 to 8 ASCII letters or digits separated by
/// '-'. The engine matches tags case-insensitively.
fn is_language_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.split('-').all(|subtag| {
            (1..=8).contains(&subtag.len()) && subtag.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

fn map_dash_video_quality(quality: MesioDashVideoQuality) -> VideoSelection {
    match quality {
        MesioDashVideoQuality::Highest => VideoSelection::Highest,
        MesioDashVideoQuality::Lowest => VideoSelection::Lowest,
        MesioDashVideoQuality::MaxHeight { height } => VideoSelection::MaxHeight(height),
        MesioDashVideoQuality::ClosestBandwidth { bandwidth } => {
            VideoSelection::ClosestBandwidth(bandwidth)
        }
    }
}

fn map_dash_audio_quality(quality: MesioDashAudioQuality) -> AudioSelection {
    match quality {
        MesioDashAudioQuality::Highest => AudioSelection::Highest,
        MesioDashAudioQuality::Lowest => AudioSelection::Lowest,
        MesioDashAudioQuality::ClosestBandwidth { bandwidth } => {
            AudioSelection::ClosestBandwidth(bandwidth)
        }
    }
}

fn map_dash_live_start(live_start: MesioDashLiveStart) -> LiveStartPolicy {
    match live_start {
        MesioDashLiveStart::Auto => LiveStartPolicy::PresentationDelay(None),
        MesioDashLiveStart::LiveEdge => LiveStartPolicy::LiveEdge,
        MesioDashLiveStart::Delay { delay_ms } => {
            LiveStartPolicy::PresentationDelay(Some(Duration::from_millis(delay_ms)))
        }
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
        debug!("Adding header: {}", key);
        builder = builder.add_header(key, value);
    }

    // Map cookies as a Cookie header
    if let Some(ref cookies) = config.cookies {
        builder = builder.add_header("Cookie", cookies);
    }

    // A direct route turns off the environment's proxy too.
    let (proxy, use_system_proxy) = match &config.proxy {
        ProxyTarget::Explicit(endpoint) => (Some(mesio_proxy(endpoint)), false),
        ProxyTarget::System => (None, true),
        ProxyTarget::Direct => (None, false),
    };
    builder = builder.with_config(|cfg| {
        cfg.base.proxy = proxy;
        cfg.base.use_system_proxy = use_system_proxy;
    });

    builder.get_config()
}

/// Mesio's form of a proxy: the address, the protocol spoken to the proxy,
/// and the login.
fn mesio_proxy(endpoint: &ProxyEndpoint) -> ProxyConfig {
    let url = endpoint.url.trim();
    let scheme = url
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    let proxy_type = match scheme.as_deref() {
        Some("socks5" | "socks5h") => ProxyType::Socks5,
        Some("https") => ProxyType::Https,
        _ => ProxyType::Http,
    };
    ProxyConfig {
        url: url.to_owned(),
        proxy_type,
        auth: endpoint.login().map(|(username, password)| ProxyAuth {
            username: username.to_owned(),
            password: password.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flv_fix::FlvPipelineConfig;
    use hls_fix::HlsPipelineConfig;
    use pipeline_common::config::PipelineConfig;
    use std::path::PathBuf;

    use crate::downloader::DownloadProtocol;

    fn create_test_download_config() -> DownloadConfig {
        DownloadConfig {
            managed_credentials: false,
            credential_binding: None,
            url: "https://example.com/stream.m3u8".to_string(),
            output_dir: PathBuf::from("/tmp/downloads"),
            filename_template: "test-stream".to_string(),
            output_format: "ts".to_string(),
            max_segment_duration_secs: 0,
            max_segment_size_bytes: 0,
            proxy: ProxyTarget::Direct,
            cookies: None,
            headers: Vec::new(),
            streamer_id: "test-streamer".to_string(),
            streamer_name: "test-streamer".to_string(),
            session_id: "test-session".to_string(),
            initial_segment_index: 0,
            protocol: DownloadProtocol::Hls,
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
        config.proxy =
            ProxyTarget::Explicit(ProxyEndpoint::new("http://proxy.example.com:8080", None));

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

    fn endpoint(url: &str, login: Option<(&str, &str)>) -> ProxyEndpoint {
        ProxyEndpoint::new(
            url,
            login.map(|(user, pass)| (user.to_owned(), pass.to_owned())),
        )
    }

    #[test]
    fn mesio_speaks_the_proxy_scheme() {
        for (url, proxy_type) in [
            ("http://proxy.example.com:8080", ProxyType::Http),
            ("https://secure-proxy.example.com:443", ProxyType::Https),
            ("socks5://socks-proxy.example.com:1080", ProxyType::Socks5),
            ("SOCKS5H://socks-proxy.example.com:1080", ProxyType::Socks5),
        ] {
            let proxy = mesio_proxy(&endpoint(url, None));
            assert_eq!(proxy.proxy_type, proxy_type, "{url}");
            assert_eq!(proxy.url, url);
            assert!(proxy.auth.is_none());
        }
    }

    #[test]
    fn mesio_receives_the_login_as_written() {
        let proxy = mesio_proxy(&endpoint(
            "socks5h://proxy.example:1080",
            Some(("us@er:x/", "p%20ss w@rd")),
        ));
        assert_eq!(proxy.url, "socks5h://proxy.example:1080");
        let auth = proxy.auth.unwrap();
        assert_eq!(
            (auth.username.as_str(), auth.password.as_str()),
            ("us@er:x/", "p%20ss w@rd")
        );
        assert!(
            !format!(
                "{:?}",
                mesio_proxy(&endpoint(
                    "http://proxy.example:8080",
                    Some(("user", "secret"))
                ))
            )
            .contains("secret")
        );
    }

    #[test]
    fn test_build_pipeline_config_default() {
        let config = create_test_download_config();
        let pipeline_config = config.build_pipeline_config();

        assert_eq!(pipeline_config.channel_size, 64);
        assert_eq!(pipeline_config.max_file_size, 0);
    }

    #[test]
    fn test_build_pipeline_config_with_max_duration() {
        let mut config = create_test_download_config();
        config.max_segment_duration_secs = 3600;

        let pipeline_config = config.build_pipeline_config();

        assert_eq!(
            pipeline_config.max_duration,
            Some(std::time::Duration::from_secs(3600))
        );
    }

    #[test]
    fn test_build_hls_pipeline_config_default() {
        let config = create_test_download_config();
        let hls_pipeline_config = config.build_hls_pipeline_config();

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
        let flv_pipeline_config = config.build_flv_pipeline_config();

        // Should return default config - check individual fields
        let default_config = FlvPipelineConfig::default();
        assert!(!flv_pipeline_config.duplicate_tag_filtering);
        assert!(
            !flv_pipeline_config
                .duplicate_tag_filter_config
                .enable_replay_offset_matching
        );
        assert_eq!(flv_pipeline_config.pipe_mode, default_config.pipe_mode);
    }

    #[test]
    fn test_build_pipeline_config_with_max_size() {
        let mut config = create_test_download_config();
        config.max_segment_size_bytes = 1024 * 1024 * 100; // 100 MB

        let pipeline_config = config.build_pipeline_config();

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

        let pipeline_config = config.build_pipeline_config();

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

        let hls_pipeline_config = config.build_hls_pipeline_config();

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
                .pipe_mode(true)
                .build(),
        );

        let flv_pipeline_config = config.build_flv_pipeline_config();

        assert!(!flv_pipeline_config.duplicate_tag_filtering);
        assert!(flv_pipeline_config.pipe_mode);
    }

    #[test]
    fn test_build_hls_config_applies_engine_overrides() {
        let config = create_test_download_config();
        let engine_config: MesioEngineConfig = serde_json::from_str(
            r#"
            {
              "hls": {
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
                  "max_segment_retry_delay_ms": 5555,
                  "max_key_retry_delay_ms": 6666,
                  "progress_emit_min_bytes": 0,
                  "progress_emit_min_interval_ms": 0
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
        assert_eq!(
            hls_config.fetcher_config.max_segment_retry_delay,
            std::time::Duration::from_millis(5555)
        );
        assert_eq!(
            hls_config.fetcher_config.max_key_retry_delay,
            std::time::Duration::from_millis(6666)
        );
        assert_eq!(hls_config.fetcher_config.progress_emit_min_bytes, 0);
        assert_eq!(
            hls_config.fetcher_config.progress_emit_min_interval,
            std::time::Duration::ZERO
        );
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
    }

    fn dash_engine_config(dash: serde_json::Value) -> MesioEngineConfig {
        serde_json::from_value(serde_json::json!({ "dash": dash })).unwrap()
    }

    #[test]
    fn build_dash_config_maps_request_settings() {
        let mut config = create_test_download_config();
        config.url = "https://example.com/live.mpd".to_string();
        config.headers = vec![("Referer".to_string(), "https://example.com".to_string())];
        config.cookies = Some("auth=secret".to_string());
        config.proxy =
            ProxyTarget::Explicit(ProxyEndpoint::new("socks5://proxy.example.com:1080", None));

        let dash = build_dash_config(&config, None, &MesioEngineConfig::default()).unwrap();

        let header = |name| {
            dash.base
                .headers
                .get(name)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        assert_eq!(
            header(reqwest::header::REFERER).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            header(reqwest::header::COOKIE).as_deref(),
            Some("auth=secret")
        );
        let proxy = dash.base.proxy.unwrap();
        assert_eq!(proxy.proxy_type, ProxyType::Socks5);

        let direct = build_dash_config(
            &create_test_download_config(),
            None,
            &MesioEngineConfig::default(),
        )
        .unwrap();
        assert!(direct.base.proxy.is_none());
        assert!(!direct.base.use_system_proxy);
    }

    #[test]
    fn build_dash_config_without_overrides_keeps_engine_defaults() {
        let config = create_test_download_config();
        let dash = build_dash_config(&config, None, &MesioEngineConfig::default()).unwrap();
        let engine = DashConfig::default();
        assert_eq!(dash.selection.video, engine.selection.video);
        assert_eq!(dash.live_start, engine.live_start);
        assert_eq!(
            dash.scheduler.download_concurrency,
            engine.scheduler.download_concurrency
        );

        // An empty `dash` object is the same as none.
        let empty =
            build_dash_config(&config, None, &dash_engine_config(serde_json::json!({}))).unwrap();
        assert_eq!(empty.selection.video, engine.selection.video);
        assert_eq!(empty.manifest.fetch_timeout, engine.manifest.fetch_timeout);
        assert_eq!(
            empty.fetcher.max_segment_retries,
            engine.fetcher.max_segment_retries
        );
    }

    #[test]
    fn build_dash_config_applies_engine_overrides() {
        let config = create_test_download_config();
        let engine_config = dash_engine_config(serde_json::json!({
            "video_quality": { "type": "max_height", "height": 720 },
            "audio_quality": { "type": "lowest" },
            "audio_language": " pt-BR ",
            "include_video": true,
            "include_audio": false,
            "download_concurrency": 2,
            "manifest_timeout_ms": 3000,
            "min_refresh_interval_ms": 500,
            "max_refresh_retries": 9,
            "segment_timeout_ms": 4000,
            "max_segment_retries": 7,
            "live_start": { "type": "delay", "delay_ms": 6000 }
        }));

        let dash = build_dash_config(&config, None, &engine_config).unwrap();

        assert_eq!(dash.selection.video, VideoSelection::MaxHeight(720));
        assert_eq!(dash.selection.audio, AudioSelection::Lowest);
        assert_eq!(dash.selection.audio_language.as_deref(), Some("pt-BR"));
        assert!(dash.selection.include_video);
        assert!(!dash.selection.include_audio);
        assert_eq!(dash.scheduler.download_concurrency, 2);
        assert_eq!(dash.manifest.fetch_timeout, Duration::from_millis(3000));
        assert_eq!(
            dash.manifest.min_refresh_interval,
            Duration::from_millis(500)
        );
        assert_eq!(dash.manifest.max_refresh_retries, 9);
        assert_eq!(
            dash.fetcher.segment_download_timeout,
            Duration::from_millis(4000)
        );
        assert_eq!(dash.fetcher.max_segment_retries, 7);
        assert_eq!(
            dash.live_start,
            LiveStartPolicy::PresentationDelay(Some(Duration::from_millis(6000)))
        );

        let edge = dash_engine_config(serde_json::json!({
            "video_quality": { "type": "closest_bandwidth", "bandwidth": 3000000 },
            "live_start": { "type": "live_edge" }
        }));
        let dash = build_dash_config(&config, None, &edge).unwrap();
        assert_eq!(
            dash.selection.video,
            VideoSelection::ClosestBandwidth(3_000_000)
        );
        assert_eq!(dash.live_start, LiveStartPolicy::LiveEdge);
    }

    #[test]
    fn build_dash_config_rejects_unusable_settings_as_configuration_errors() {
        let config = create_test_download_config();
        for dash in [
            serde_json::json!({ "include_video": false, "include_audio": false }),
            serde_json::json!({ "download_concurrency": 0 }),
            serde_json::json!({ "manifest_timeout_ms": 0 }),
            serde_json::json!({ "min_refresh_interval_ms": 0 }),
            serde_json::json!({ "segment_timeout_ms": 0 }),
            serde_json::json!({ "video_quality": { "type": "max_height", "height": 0 } }),
            serde_json::json!({ "video_quality": { "type": "closest_bandwidth", "bandwidth": 0 } }),
            serde_json::json!({ "audio_quality": { "type": "closest_bandwidth", "bandwidth": 0 } }),
            serde_json::json!({ "audio_language": "" }),
            serde_json::json!({ "audio_language": "english please" }),
            serde_json::json!({ "audio_language": "toolongsubtag" }),
        ] {
            let result = build_dash_config(&config, None, &dash_engine_config(dash.clone()));
            assert!(
                matches!(result, Err(DownloadError::Configuration { .. })),
                "{dash}"
            );
        }
    }
}
