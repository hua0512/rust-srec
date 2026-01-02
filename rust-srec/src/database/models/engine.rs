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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    utoipa::ToSchema,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
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
