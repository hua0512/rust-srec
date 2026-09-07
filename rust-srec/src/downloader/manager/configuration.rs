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
        if overrides.is_some_and(|value| !value.is_object()) {
            return Err(crate::Error::config(
                "Engine overrides must be a JSON object",
            ));
        }
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
                        .await?;
                    Arc::new(
                        FfmpegEngine::with_config_async(Self::apply_override(
                            base,
                            override_config,
                        )?)
                        .await,
                    )
                }
                EngineType::Streamlink => {
                    let base = self
                        .load_engine_config_or_default::<StreamlinkEngineConfig>(target_id)
                        .await?;
                    Arc::new(
                        StreamlinkEngine::with_config_async(Self::apply_override(
                            base,
                            override_config,
                        )?)
                        .await,
                    )
                }
                EngineType::Mesio => {
                    let base = self
                        .load_engine_config_or_default::<MesioEngineConfig>(target_id)
                        .await?;
                    Arc::new(MesioEngine::with_config(Self::apply_override(
                        base,
                        override_config,
                    )?))
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
                            EngineType::Ffmpeg => Arc::new(
                                FfmpegEngine::with_config_async(parse_engine_config(
                                    "ffmpeg",
                                    &config.config,
                                )?)
                                .await,
                            ),
                            EngineType::Streamlink => Arc::new(
                                StreamlinkEngine::with_config_async(parse_engine_config(
                                    "streamlink",
                                    &config.config,
                                )?)
                                .await,
                            ),
                            EngineType::Mesio => Arc::new(MesioEngine::with_config(
                                parse_engine_config("mesio", &config.config)?,
                            )),
                        };
                        return Ok((engine, engine_type, key));
                    }
                    Err(crate::Error::NotFound { .. }) => {
                        warn!(engine_id = id, "Engine config not found; using default")
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        let engine = self.get_engine(default_engine).ok_or_else(|| {
            crate::Error::Other(format!("Default engine {default_engine} not registered"))
        })?;
        Ok((engine, default_engine, EngineKey::global(default_engine)))
    }

    async fn load_engine_config_or_default<T>(&self, id: &str) -> Result<T>
    where
        T: DeserializeOwned + Default,
    {
        let Some(repo) = &self.config_repo else {
            return Ok(T::default());
        };

        match repo.get_engine_config(id).await {
            Ok(config) => serde_json::from_str::<T>(&config.config).map_err(|error| {
                crate::Error::config(format!(
                    "Invalid base engine configuration {id}: {:?} at line {}, column {}",
                    error.classify(),
                    error.line(),
                    error.column()
                ))
            }),
            Err(crate::Error::NotFound { .. }) => Ok(T::default()),
            Err(error) => Err(error),
        }
    }

    fn apply_override<T>(base: T, override_value: &serde_json::Value) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
    {
        if !override_value.is_object() {
            return Err(crate::Error::config(
                "Engine override must be a JSON object",
            ));
        }
        let merged = Self::merge_config_json(&base, override_value)?;
        serde_json::from_value::<T>(merged).map_err(|error| {
            crate::Error::config(format!("Invalid engine override: {:?}", error.classify()))
        })
    }

    async fn resolve_engine_type(&self, id: &str) -> Result<EngineType> {
        if let Ok(engine_type) = id.parse::<EngineType>() {
            return Ok(engine_type);
        }

        let Some(repo) = &self.config_repo else {
            return Err(crate::Error::Other(format!("Unknown engine: {id}")));
        };
        let config = repo.get_engine_config(id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::SqlxConfigRepository;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn manager_with_repository() -> (DownloadManager, sqlx::SqlitePool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE engine_configuration (id TEXT PRIMARY KEY, name TEXT NOT NULL, engine_type TEXT NOT NULL, config TEXT NOT NULL)")
            .execute(&pool).await.unwrap();
        let manager = DownloadManager::new().with_config_repo(Arc::new(SqlxConfigRepository::new(
            pool.clone(),
            pool.clone(),
        )));
        (manager, pool)
    }

    async fn insert_config(pool: &sqlx::SqlitePool, id: &str, config: &str) {
        sqlx::query("INSERT INTO engine_configuration (id, name, engine_type, config) VALUES (?, ?, 'FFMPEG', ?)")
            .bind(id).bind(id).bind(config).execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn resolution_preserves_default_custom_override_and_missing_keys() {
        let (manager, pool) = manager_with_repository().await;
        let (_, engine_type, key) = manager.resolve_engine(None, None).await.unwrap();
        assert_eq!(key, EngineKey::global(engine_type));
        let (_, missing_type, missing_key) = manager
            .resolve_engine(Some("missing-custom"), None)
            .await
            .unwrap();
        assert_eq!(missing_type, engine_type);
        assert_eq!(missing_key, key);

        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::json!({"binary_path": dir.path().join("missing-ffmpeg")});
        insert_config(&pool, "custom", &config.to_string()).await;
        let (engine, resolved, key) = manager.resolve_engine(Some("custom"), None).await.unwrap();
        assert_eq!(resolved, EngineType::Ffmpeg);
        assert_eq!(key, EngineKey::custom(EngineType::Ffmpeg, "custom"));
        assert!(!engine.is_available());

        let override_value = serde_json::json!({"timeout_secs": 7});
        let overrides = serde_json::json!({"custom": override_value});
        let (engine, resolved, key) = manager
            .resolve_engine(Some("custom"), Some(&overrides))
            .await
            .unwrap();
        assert_eq!(resolved, EngineType::Ffmpeg);
        assert_eq!(
            key,
            EngineKey::with_override(
                EngineType::Ffmpeg,
                Some("custom"),
                DownloadManager::hash_override(&override_value)
            )
        );
        assert!(
            !engine.is_available(),
            "override retains configured executable"
        );
        let global_override = serde_json::json!({"FFMPEG": config});
        let (_, resolved, _) = manager
            .resolve_engine(Some("FFMPEG"), Some(&global_override))
            .await
            .unwrap();
        assert_eq!(
            resolved,
            EngineType::Ffmpeg,
            "missing built-in config uses its typed defaults"
        );
    }

    #[tokio::test]
    async fn repository_timeouts_never_fall_back_to_default() {
        let (manager, pool) = manager_with_repository().await;
        let _occupied_connection = pool.acquire().await.unwrap();
        let overrides = serde_json::json!({"FFMPEG": {"timeout_secs": 7}});
        for (id, overrides) in [("custom", None), ("FFMPEG", Some(&overrides))] {
            let result = manager.resolve_engine(Some(id), overrides).await;
            assert!(matches!(
                result,
                Err(crate::Error::DatabaseSqlx(sqlx::Error::PoolTimedOut))
            ));
        }
        assert!(
            manager.resolve_engine(None, None).await.is_ok(),
            "unspecified engine does not require repository lookup"
        );
    }

    #[tokio::test]
    async fn malformed_base_and_overrides_fail_instead_of_resetting_configuration() {
        let (manager, pool) = manager_with_repository().await;
        insert_config(&pool, "malformed", "{").await;
        assert!(
            manager
                .resolve_engine(Some("malformed"), None)
                .await
                .is_err()
        );
        let override_value = serde_json::json!({"malformed": {"timeout_secs": 7}});
        assert!(
            manager
                .resolve_engine(Some("malformed"), Some(&override_value))
                .await
                .is_err()
        );
        for overrides in [
            serde_json::json!([]),
            serde_json::json!({"FFMPEG": []}),
            serde_json::json!({"FFMPEG": {"timeout_secs": "private-token"}}),
            serde_json::json!({"FFMPEG": {"binary_path": 42}}),
        ] {
            let error = match manager
                .resolve_engine(Some("FFMPEG"), Some(&overrides))
                .await
            {
                Ok(_) => panic!("invalid override was accepted"),
                Err(error) => error,
            };
            assert!(!error.to_string().contains("private-token"));
        }
    }

    #[test]
    fn valid_override_uses_effective_configuration_without_stale_values() {
        let base = FfmpegEngineConfig {
            binary_path: "first-executable".to_owned(),
            timeout_secs: 30,
            ..Default::default()
        };
        let first = DownloadManager::apply_override(
            base.clone(),
            &serde_json::json!({"binary_path": "second-executable", "timeout_secs": 7}),
        )
        .unwrap();
        assert_eq!(first.binary_path, "second-executable");
        assert_eq!(first.timeout_secs, 7);
        let second =
            DownloadManager::apply_override(base, &serde_json::json!({"timeout_secs": 9})).unwrap();
        assert_eq!(second.binary_path, "first-executable");
        assert_eq!(second.timeout_secs, 9);
    }
}
