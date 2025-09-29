use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;
use tracing::{error, warn};

/// Represents the type of download engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineType {
    Ffmpeg,
    Streamlink,
    Mesio,
}

/// A reusable, named configuration for a download engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineConfiguration {
    pub id: Uuid,
    pub name: String,
    pub engine_type: EngineType,
    pub config: EngineConfig,
}

/// An enum that holds the specific configuration for a given engine type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EngineConfig {
    Ffmpeg(FfmpegConfig),
    Streamlink(StreamlinkConfig),
    Mesio(MesioConfig),
}

/// Configuration specific to the Ffmpeg download engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FfmpegConfig {
    pub default_args: Option<String>,
}

/// Configuration specific to the Streamlink download engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamlinkConfig {
    pub default_args: Option<String>,
}

/// Configuration specific to the Mesio download engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MesioConfig {
    pub timeout_ms: Option<u64>,
}

impl From<crate::database::models::EngineConfiguration> for EngineConfiguration {
    fn from(model: crate::database::models::EngineConfiguration) -> Self {
        let engine_type = match model.engine_type.as_str() {
            "Ffmpeg" => EngineType::Ffmpeg,
            "Streamlink" => EngineType::Streamlink,
            "Mesio" => EngineType::Mesio,
            unknown => {
                warn!("Unknown engine type: {}. Defaulting to Ffmpeg.", unknown);
                EngineType::Ffmpeg
            }
        };

        let config = match engine_type {
            EngineType::Ffmpeg => EngineConfig::Ffmpeg(
                serde_json::from_value(model.config.clone()).unwrap_or_else(|e| {
                    error!("Failed to parse FfmpegConfig: {}. Using default.", e);
                    FfmpegConfig { default_args: None }
                }),
            ),
            EngineType::Streamlink => EngineConfig::Streamlink(
                serde_json::from_value(model.config.clone()).unwrap_or_else(|e| {
                    error!("Failed to parse StreamlinkConfig: {}. Using default.", e);
                    StreamlinkConfig { default_args: None }
                }),
            ),
            EngineType::Mesio => EngineConfig::Mesio(
                serde_json::from_value(model.config.clone()).unwrap_or_else(|e| {
                    error!("Failed to parse MesioConfig: {}. Using default.", e);
                    MesioConfig { timeout_ms: None }
                }),
            ),
        };

        Self {
            id: Uuid::parse_str(&model.id).unwrap_or_else(|e| {
                error!("Failed to parse UUID from string '{}': {}", &model.id, e);
                Uuid::nil()
            }),
            name: model.name,
            engine_type,
            config,
        }
    }
}
