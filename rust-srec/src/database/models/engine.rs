//! Engine configuration database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

pub use crate::domain::engine_config::{
    EngineType, FfmpegEngineConfig, MesioBufferLimitsOverride, MesioDashAudioQuality,
    MesioDashConfig, MesioDashLiveStart, MesioDashVideoQuality, MesioDownloaderBaseOverride,
    MesioDuplicateTagFilterConfig, MesioEngineConfig, MesioFlvConfig, MesioFlvFixConfig,
    MesioGapSkipStrategy, MesioHlsCacheConfigOverride, MesioHlsConfig,
    MesioHlsDecryptionConfigOverride, MesioHlsFetcherConfigOverride, MesioHlsOutputConfigOverride,
    MesioHlsPlaylistConfigOverride, MesioHlsProcessorConfigOverride,
    MesioHlsSchedulerConfigOverride, MesioHlsVariantSelectionPolicy, MesioHttpVersionPreference,
    MesioSequenceHeaderChangeMode, StreamlinkEngineConfig,
};

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
