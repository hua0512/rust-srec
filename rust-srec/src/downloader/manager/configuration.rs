use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use crate::Result;
use crate::database::models::engine::{
    FfmpegEngineConfig, MesioEngineConfig, StreamlinkEngineConfig,
};
#[cfg(test)]
use crate::downloader::engine::DownloadConfig;
use crate::downloader::engine::{
    DownloadEngine, EngineStartError, EngineType, FfmpegEngine, MesioEngine, StreamlinkEngine,
};
use crate::downloader::resilience::EngineKey;

use super::{DownloadManager, io_error_in_chain, parse_engine_config};

impl DownloadManager {
    /// Prepare the output directory before starting an engine.
    #[cfg(test)]
    pub(super) async fn prepare_output_dir(
        &self,
        config: &DownloadConfig,
    ) -> std::result::Result<(), EngineStartError> {
        self.prepare_output_dir_for_path(&config.output_dir).await
    }

    /// Prepares `output_dir` and records output-root gate failures and recoveries.
    pub(super) async fn prepare_output_dir_for_path(
        &self,
        output_dir: &std::path::Path,
    ) -> std::result::Result<(), EngineStartError> {
        match crate::downloader::engine::utils::ensure_output_dir(output_dir).await {
            Ok(()) => {
                if let Some(gate) = self.output_root_gate.get() {
                    gate.mark_healthy(output_dir);
                }
                Ok(())
            }
            Err(crate_err) => {
                if let Some(gate) = self.output_root_gate.get()
                    && let Some(io_err) = io_error_in_chain(&crate_err)
                {
                    gate.record_failure(output_dir, io_err);
                }
                Err(EngineStartError::from(crate_err))
            }
        }
    }

    /// Resolves an engine instance, its type, and its circuit-breaker key.
    pub(super) async fn resolve_engine(
        &self,
        engine_id: Option<&str>,
        overrides: Option<&serde_json::Value>,
    ) -> Result<(Arc<dyn DownloadEngine>, EngineType, EngineKey)> {
        let default_engine = self.config.read().default_engine;
        let target_id = engine_id.unwrap_or(default_engine.as_str());
        let specific_override = overrides.and_then(|value| value.get(target_id));

        if let Some(override_config) = specific_override {
            debug!(engine_id = target_id, "Applying engine override");
            let override_hash = Self::hash_override(override_config);
            let engine_type = self.resolve_engine_type(target_id).await?;
            let key = EngineKey::with_override(engine_type, engine_id, override_hash);

            let engine: Arc<dyn DownloadEngine> = match engine_type {
                EngineType::Ffmpeg => {
                    let base = self
                        .load_engine_config_or_default::<FfmpegEngineConfig>(target_id)
                        .await;
                    Arc::new(FfmpegEngine::with_config(Self::apply_override_best_effort(
                        base,
                        override_config,
                    )))
                }
                EngineType::Streamlink => {
                    let base = self
                        .load_engine_config_or_default::<StreamlinkEngineConfig>(target_id)
                        .await;
                    Arc::new(StreamlinkEngine::with_config(
                        Self::apply_override_best_effort(base, override_config),
                    ))
                }
                EngineType::Mesio => {
                    let base = self
                        .load_engine_config_or_default::<MesioEngineConfig>(target_id)
                        .await;
                    Arc::new(MesioEngine::with_config(Self::apply_override_best_effort(
                        base,
                        override_config,
                    )))
                }
            };

            return Ok((engine, engine_type, key));
        }

        if let Some(id) = engine_id {
            if let Ok(known_type) = id.parse::<EngineType>() {
                let engine = self.get_engine(known_type).ok_or_else(|| {
                    crate::Error::Other(format!("Default engine {known_type} not registered"))
                })?;
                return Ok((engine, known_type, EngineKey::global(known_type)));
            }

            if let Some(repo) = &self.config_repo {
                match repo.get_engine_config(id).await {
                    Ok(config) => {
                        let engine_type =
                            config.engine_type.parse::<EngineType>().map_err(|_| {
                                crate::Error::Other(format!(
                                    "Unknown engine type in config: {}",
                                    config.engine_type
                                ))
                            })?;
                        let key = EngineKey::custom(engine_type, id);
                        let engine: Arc<dyn DownloadEngine> = match engine_type {
                            EngineType::Ffmpeg => Arc::new(FfmpegEngine::with_config(
                                parse_engine_config("ffmpeg", &config.config)?,
                            )),
                            EngineType::Streamlink => Arc::new(StreamlinkEngine::with_config(
                                parse_engine_config("streamlink", &config.config)?,
                            )),
                            EngineType::Mesio => Arc::new(MesioEngine::with_config(
                                parse_engine_config("mesio", &config.config)?,
                            )),
                        };
                        return Ok((engine, engine_type, key));
                    }
                    Err(_) => warn!(engine_id = id, "Engine config not found; using default"),
                }
            }
        }

        let engine = self.get_engine(default_engine).ok_or_else(|| {
            crate::Error::Other(format!("Default engine {default_engine} not registered"))
        })?;
        Ok((engine, default_engine, EngineKey::global(default_engine)))
    }

    async fn load_engine_config_or_default<T>(&self, id: &str) -> T
    where
        T: DeserializeOwned + Default,
    {
        let Some(repo) = &self.config_repo else {
            return T::default();
        };

        match repo.get_engine_config(id).await {
            Ok(config) => serde_json::from_str::<T>(&config.config).unwrap_or_default(),
            Err(_) => T::default(),
        }
    }

    fn apply_override_best_effort<T>(mut base: T, override_value: &serde_json::Value) -> T
    where
        T: Serialize + DeserializeOwned,
    {
        if let Ok(merged) = Self::merge_config_json(&base, override_value)
            && let Ok(updated) = serde_json::from_value::<T>(merged)
        {
            base = updated;
        }
        base
    }

    async fn resolve_engine_type(&self, id: &str) -> Result<EngineType> {
        if let Ok(engine_type) = id.parse::<EngineType>() {
            return Ok(engine_type);
        }

        let Some(repo) = &self.config_repo else {
            return Err(crate::Error::Other(format!("Unknown engine: {id}")));
        };
        let config = repo
            .get_engine_config(id)
            .await
            .map_err(|_| crate::Error::Other(format!("Unknown engine: {id}")))?;
        config.engine_type.parse::<EngineType>().map_err(|_| {
            crate::Error::Other(format!("Unknown engine type: {}", config.engine_type))
        })
    }

    fn merge_config_json<T: Serialize>(
        base: &T,
        override_value: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut base_value =
            serde_json::to_value(base).map_err(|error| crate::Error::Other(error.to_string()))?;
        Self::json_merge(&mut base_value, override_value);
        Ok(base_value)
    }

    fn json_merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
        if let serde_json::Value::Object(patch_map) = patch {
            if !target.is_object() {
                *target = serde_json::Value::Object(serde_json::Map::new());
            }
            if let Some(target_map) = target.as_object_mut() {
                for (key, value) in patch_map {
                    if value.is_null() {
                        target_map.remove(key);
                    } else if let Some(existing) = target_map.get_mut(key) {
                        Self::json_merge(existing, value);
                    } else {
                        target_map.insert(key.clone(), value.clone());
                    }
                }
            }
        } else {
            *target = patch.clone();
        }
    }

    fn hash_override(override_value: &serde_json::Value) -> u64 {
        use std::hash::{Hash, Hasher};

        let canonical = Self::canonicalize_json(override_value);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.to_string().hash(&mut hasher);
        hasher.finish()
    }

    fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut canonical = serde_json::Map::with_capacity(map.len());
                for key in keys {
                    if let Some(child) = map.get(key) {
                        canonical.insert(key.clone(), Self::canonicalize_json(child));
                    }
                }
                serde_json::Value::Object(canonical)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(Self::canonicalize_json).collect())
            }
            _ => value.clone(),
        }
    }
}
