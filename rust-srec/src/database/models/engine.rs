//! Engine configuration database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Engine configuration database model.
/// Stores a named, reusable engine configuration.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EngineConfigurationDbModel {
    pub id: String,
    pub name: String,
    /// Engine type: FFMPEG, STREAMLINK, MESIO
    pub engine_type: String,
    /// JSON blob for engine-specific configuration
    pub config: String,
}

impl EngineConfigurationDbModel {
    pub fn new(
        name: impl Into<String>,
        engine_type: EngineType,
        config: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            engine_type: engine_type.as_str().to_string(),
            config: config.into(),
        }
    }
}

/// Engine types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EngineType {
    Ffmpeg,
    Streamlink,
    Mesio,
}

impl EngineType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ffmpeg => "FFMPEG",
            Self::Streamlink => "STREAMLINK",
            Self::Mesio => "MESIO",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "FFMPEG" => Some(Self::Ffmpeg),
            "STREAMLINK" => Some(Self::Streamlink),
            "MESIO" => Some(Self::Mesio),
            _ => None,
        }
    }
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EngineType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "FFMPEG" => Ok(Self::Ffmpeg),
            "STREAMLINK" => Ok(Self::Streamlink),
            "MESIO" => Ok(Self::Mesio),
            _ => Err(format!("Unknown engine type: {s}")),
        }
    }
}

/// FFmpeg engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FfmpegEngineConfig {
    /// Path to ffmpeg binary
    #[serde(default = "default_ffmpeg_path")]
    pub binary_path: String,
    /// Additional input arguments
    #[serde(default)]
    pub input_args: Vec<String>,
    /// Additional output arguments
    #[serde(default)]
    pub output_args: Vec<String>,
    /// Timeout for connection in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
    /// User agent string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

fn default_ffmpeg_path() -> String {
    "ffmpeg".to_string()
}

fn default_timeout() -> u32 {
    30
}

impl Default for FfmpegEngineConfig {
    fn default() -> Self {
        Self {
            binary_path: default_ffmpeg_path(),
            input_args: Vec::new(),
            output_args: Vec::new(),
            timeout_secs: default_timeout(),
            user_agent: None,
        }
    }
}

/// Streamlink engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamlinkEngineConfig {
    /// Path to streamlink binary
    #[serde(default = "default_streamlink_path")]
    pub binary_path: String,
    /// Quality preference (e.g., "best", "720p")
    #[serde(default = "default_quality")]
    pub quality: String,
    /// Additional arguments
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Twitch proxy playlist (ttv-lol)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twitch_proxy_playlist: Option<String>,
    /// Twitch proxy playlist exclude (ttv-lol)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twitch_proxy_playlist_exclude: Option<String>,
}

fn default_streamlink_path() -> String {
    "streamlink".to_string()
}

fn default_quality() -> String {
    "best".to_string()
}

impl Default for StreamlinkEngineConfig {
    fn default() -> Self {
        Self {
            binary_path: default_streamlink_path(),
            quality: default_quality(),
            extra_args: Vec::new(),
            twitch_proxy_playlist: None,
            twitch_proxy_playlist_exclude: None,
        }
    }
}

/// How the FLV splitter should detect audio/video sequence-header changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MesioSequenceHeaderChangeMode {
    /// Split when the raw CRC32 of the sequence header changes (legacy behavior).
    Crc32,
    /// Split only when the codec configuration meaningfully changes.
    SemanticSignature,
}

/// Overrides for the FLV duplicate media-tag filter.
///
/// Fields are optional so they can be used as a partial override payload.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioDuplicateTagFilterConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_capacity_tags: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_backjump_threshold_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_replay_offset_matching: Option<bool>,
}

/// Mesio-configurable knobs for FLV fixing.
///
/// This config is applied only when FLV pipeline processing is enabled.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioFlvFixConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_header_change_mode: Option<MesioSequenceHeaderChangeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_duplicate_sequence_headers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_tag_filtering: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_tag_filter_config: Option<MesioDuplicateTagFilterConfig>,
}

impl MesioFlvFixConfig {
    pub fn apply_to(&self, cfg: &mut flv_fix::FlvPipelineConfig) {
        if let Some(mode) = self.sequence_header_change_mode {
            cfg.sequence_header_change_mode = match mode {
                MesioSequenceHeaderChangeMode::Crc32 => flv_fix::SequenceHeaderChangeMode::Crc32,
                MesioSequenceHeaderChangeMode::SemanticSignature => {
                    flv_fix::SequenceHeaderChangeMode::SemanticSignature
                }
            };
        }

        if let Some(value) = self.drop_duplicate_sequence_headers {
            cfg.drop_duplicate_sequence_headers = value;
        }

        if let Some(value) = self.duplicate_tag_filtering {
            cfg.duplicate_tag_filtering = value;
        }

        if let Some(ref override_cfg) = self.duplicate_tag_filter_config {
            let mut c = cfg.duplicate_tag_filter_config.clone();
            if let Some(value) = override_cfg.window_capacity_tags {
                c.window_capacity_tags = value;
            }
            if let Some(value) = override_cfg.replay_backjump_threshold_ms {
                c.replay_backjump_threshold_ms = value;
            }
            if let Some(value) = override_cfg.enable_replay_offset_matching {
                c.enable_replay_offset_matching = value;
            }
            cfg.duplicate_tag_filter_config = c;
        }
    }
}

/// Mesio engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesioEngineConfig {
    /// Buffer size in bytes
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
    /// Enable FLV fixing
    #[serde(default = "default_true")]
    pub fix_flv: bool,
    /// Enable HLS fixing
    #[serde(default = "default_true")]
    pub fix_hls: bool,
    /// Extra FLV-fix tuning knobs for Mesio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flv_fix: Option<MesioFlvFixConfig>,
    /// Extra HLS runtime tuning knobs for Mesio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hls: Option<MesioHlsConfig>,
}

/// Mesio HLS tuning configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsConfig {
    /// Override low-level HTTP client tuning for HLS requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<MesioDownloaderBaseOverride>,
    /// Override playlist polling/parsing behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_config: Option<MesioHlsPlaylistConfigOverride>,
    /// Override scheduler settings (download concurrency, buffers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_config: Option<MesioHlsSchedulerConfigOverride>,
    /// Override segment/key fetcher settings (timeouts/retries/streaming threshold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetcher_config: Option<MesioHlsFetcherConfigOverride>,
    /// Override processor settings (processed segment cache TTL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processor_config: Option<MesioHlsProcessorConfigOverride>,
    /// Override decryption settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decryption_config: Option<MesioHlsDecryptionConfigOverride>,
    /// Override cache TTLs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_config: Option<MesioHlsCacheConfigOverride>,
    /// Override output buffering, gap handling, stall timeouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config: Option<MesioHlsOutputConfigOverride>,
    /// Override performance-related toggles (prefetch/batching/zero-copy/etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_config: Option<MesioHlsPerformanceConfigOverride>,

    /// Override live gap-skip strategy (to reduce false skips from out-of-order arrivals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_gap_strategy: Option<MesioGapSkipStrategy>,
    /// Override Mesio HLS prefetch configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefetch: Option<MesioPrefetchOverride>,
    /// Override Mesio HLS download concurrency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_concurrency: Option<usize>,
}

/// Serializable HTTP version preference for Mesio downloader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MesioHttpVersionPreference {
    Auto,
    Http2Only,
    Http1Only,
}

/// Low-level HTTP client overrides for Mesio downloader.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioDownloaderBaseOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger_accept_invalid_certs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_ipv4: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_ipv6: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<MesioHttpVersionPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2_keep_alive_interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_max_idle_per_host: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_idle_timeout_ms: Option<u64>,
}

/// Serializable HLS variant selection policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MesioHlsVariantSelectionPolicy {
    HighestBitrate,
    LowestBitrate,
    ClosestToBitrate { target_bitrate: u64 },
    AudioOnly,
    VideoOnly,
    MatchingResolution { width: u32, height: u32 },
    Custom { value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsPlaylistConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_playlist_fetch_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_refresh_interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_max_refresh_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_refresh_retry_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_selection_policy: Option<MesioHlsVariantSelectionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptive_refresh_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptive_refresh_min_interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptive_refresh_max_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsSchedulerConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processed_segment_buffer_multiplier: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsFetcherConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_download_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_segment_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_retry_delay_base_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_segment_retry_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_download_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_key_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_retry_delay_base_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_key_retry_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_raw_cache_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_threshold_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsProcessorConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processed_segment_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsDecryptionConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_cache_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offload_decryption_to_cpu_pool: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsCacheConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decryption_key_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioBufferLimitsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_segments: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsOutputConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_reorder_buffer_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_reorder_buffer_max_segments: Option<usize>,
    /// How often to wake up and re-evaluate gap policies when stalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_evaluation_interval_ms: Option<u64>,
    /// Maximum number of pending fMP4 init segments to keep (`0` disables the limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pending_init_segments: Option<usize>,

    #[serde(
        default,
        deserialize_with = "crate::utils::json::deserialize_field_present_nullable"
    )]
    pub live_max_overall_stall_duration_ms: Option<Option<u64>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_gap_strategy: Option<MesioGapSkipStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vod_gap_strategy: Option<MesioGapSkipStrategy>,

    #[serde(
        default,
        deserialize_with = "crate::utils::json::deserialize_field_present_nullable"
    )]
    pub vod_segment_timeout_ms: Option<Option<u64>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer_limits: Option<MesioBufferLimitsOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioHlsPerformanceConfigOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decryption_offload_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefetch: Option<MesioPrefetchOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_scheduler: Option<MesioBatchSchedulerOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_copy_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioBatchSchedulerOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_window_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batch_size: Option<usize>,
}

/// Serializable gap-skip strategy for Mesio HLS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MesioGapSkipStrategy {
    WaitIndefinitely,
    SkipAfterCount { count: u64 },
    SkipAfterDuration { duration_ms: u64 },
    SkipAfterBoth { count: u64, duration_ms: u64 },
}

/// Partial override for Mesio HLS prefetch.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MesioPrefetchOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefetch_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_buffer_before_skip: Option<usize>,
}

fn default_buffer_size() -> usize {
    8 * 1024 * 1024 // 8MB
}

fn default_true() -> bool {
    true
}

impl Default for MesioEngineConfig {
    fn default() -> Self {
        Self {
            buffer_size: default_buffer_size(),
            fix_flv: true,
            fix_hls: true,
            flv_fix: None,
            hls: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_type() {
        assert_eq!(EngineType::Ffmpeg.as_str(), "FFMPEG");
        assert_eq!(EngineType::parse("MESIO"), Some(EngineType::Mesio));
    }

    #[test]
    fn test_ffmpeg_config_default() {
        let config = FfmpegEngineConfig::default();
        assert_eq!(config.binary_path, "ffmpeg");
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_engine_config_serialization() {
        let config = FfmpegEngineConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: FfmpegEngineConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.binary_path, config.binary_path);
    }

    #[test]
    fn test_mesio_config_backward_compatible() {
        let json = r#"{"buffer_size":123,"fix_flv":true,"fix_hls":false}"#;
        let parsed: MesioEngineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.buffer_size, 123);
        assert!(parsed.fix_flv);
        assert!(!parsed.fix_hls);
        assert!(parsed.flv_fix.is_none());
        assert!(parsed.hls.is_none());
    }

    #[test]
    fn test_mesio_hls_gap_strategy_parse() {
        let json = r#"
        {
          "hls": {
            "live_gap_strategy": {
              "type": "skip_after_both",
              "count": 10,
              "duration_ms": 1000
            },
            "download_concurrency": 3,
            "prefetch": {
              "enabled": true,
              "prefetch_count": 4,
              "max_buffer_before_skip": 20
            }
          }
        }"#;
        let parsed: MesioEngineConfig = serde_json::from_str(json).unwrap();
        let hls = parsed.hls.unwrap();
        assert!(matches!(
            hls.live_gap_strategy,
            Some(MesioGapSkipStrategy::SkipAfterBoth {
                count: 10,
                duration_ms: 1000
            })
        ));
        assert_eq!(hls.download_concurrency, Some(3));
        let prefetch = hls.prefetch.unwrap();
        assert_eq!(prefetch.enabled, Some(true));
        assert_eq!(prefetch.prefetch_count, Some(4));
        assert_eq!(prefetch.max_buffer_before_skip, Some(20));
    }

    #[test]
    fn test_mesio_flv_fix_config_apply() {
        let json = r#"
        {
          "flv_fix": {
            "sequence_header_change_mode": "semantic_signature",
            "drop_duplicate_sequence_headers": true,
            "duplicate_tag_filtering": false,
            "duplicate_tag_filter_config": {
              "window_capacity_tags": 123,
              "replay_backjump_threshold_ms": 5000,
              "enable_replay_offset_matching": false
            }
          }
        }"#;
        let parsed: MesioEngineConfig = serde_json::from_str(json).unwrap();
        let opts = parsed.flv_fix.unwrap();

        let mut cfg = flv_fix::FlvPipelineConfig::default();
        opts.apply_to(&mut cfg);

        assert_eq!(
            cfg.sequence_header_change_mode,
            flv_fix::SequenceHeaderChangeMode::SemanticSignature
        );
        assert!(cfg.drop_duplicate_sequence_headers);
        assert!(!cfg.duplicate_tag_filtering);
        assert_eq!(cfg.duplicate_tag_filter_config.window_capacity_tags, 123);
        assert_eq!(
            cfg.duplicate_tag_filter_config.replay_backjump_threshold_ms,
            5000
        );
        assert!(
            !cfg.duplicate_tag_filter_config
                .enable_replay_offset_matching
        );
    }
}
