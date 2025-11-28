//! Engine configuration database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Engine configuration database model.
/// Stores a named, reusable engine configuration.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
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
}
