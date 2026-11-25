use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;

mod global_platforms;
mod notifications_users;
mod presets_templates;
mod profiles;
mod proxies;
mod snapshot;
mod streamers;
#[cfg(test)]
mod tests;

use self::global_platforms::{apply_engines, apply_platforms, global_model};
use self::notifications_users::apply_notification_channels;
use self::presets_templates::{
    apply_job_presets, apply_pipeline_presets, apply_templates, delete_omitted_presets,
    delete_unimported_templates,
};
pub(super) use self::profiles::export_profiles;
use self::snapshot::{ImportSnapshot, includes_imported_users, validate_import, validate_unique};
use self::streamers::{ImportedScopes, StreamerImportDiff, apply_streamers};
use crate::config::ConfigService;
use crate::config::backup::{ConfigExport, ImportMode, ImportStats, UserExport};
use crate::database::repositories::config::{
    SqlxConfigRepository, import_global as persist_global,
};
use crate::database::repositories::proxies::RouteOwner;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::notification::NotificationService;
use crate::recorder::{ActorRegistry, OBSERVE_RETIREMENT};
use crate::streamer::StreamerManager;
use crate::streamer::state_store::{StateChange, StatePublication};

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamerManager = StreamerManager<SqlxStreamerRepository>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigurationImportError {
    #[error("{0}")]
    Validation(String),
    /// A route names a saved proxy that neither the bundle nor the
    /// installation has.
    #[error(transparent)]
    MissingProxy(crate::proxies::ProxyError),
    #[error("configuration import database operation failed: {0}")]
    Database(#[from] sqlx::Error),
}

pub(crate) struct ConfigurationImportOutcome {
    pub stats: ImportStats,
    pub warnings: Vec<String>,
}

impl From<crate::Error> for ConfigurationImportError {
    fn from(error: crate::Error) -> Self {
        match error {
            crate::Error::DatabaseSqlx(error) => Self::Database(error),
            error => Self::Database(sqlx::Error::Protocol(error.to_string())),
        }
    }
}

struct CommittedImport {
    stats: ImportStats,
    streamers: StreamerImportDiff,
    moved_proxies: Vec<String>,
    kept_presets: Vec<String>,
    retained_definitions: i64,
    credential_retirement_streamers: Vec<String>,
}

/// Single-connection transactional hook for user import validation, user
/// persistence, and auth session/refresh-token invalidation.
///
/// Injected from the top-level application composition layer so `services`
/// keeps zero dependency on the API auth store while committing user changes
/// and token invalidation in the same SQLite transaction as configuration
/// writes.
#[async_trait::async_trait]
pub(crate) trait AuthImportTxPort: Send + Sync {
    /// Validate imported user credentials (such as PHC password hash format)
    /// prior to opening the import transaction.
    fn validate_user_exports(&self, users: &[UserExport]) -> Result<(), ConfigurationImportError>;

    /// Validate imported user IDs and emails against persisted `users` rows
    /// inside `tx` before any configuration table is modified.
    async fn validate_users_tx(
        &self,
        tx: &mut sqlx::SqliteConnection,
        users: &[UserExport],
        include_users: bool,
        mode: ImportMode,
    ) -> Result<(), ConfigurationImportError>;

    /// Persist imported users (when `include_users` is true) and invalidate
    /// all auth sessions and refresh tokens inside `tx`.
    async fn apply_auth_import_tx(
        &self,
        tx: &mut sqlx::SqliteConnection,
        users: &[UserExport],
        include_users: bool,
        mode: ImportMode,
        stats: &mut ImportStats,
    ) -> Result<(), ConfigurationImportError>;
}

#[derive(Clone)]
pub(crate) struct ConfigurationImportService {
    write_pool: SqlitePool,
    admission: Arc<tokio::sync::Semaphore>,
    config_service: Arc<RuntimeConfigService>,
    streamer_manager: Arc<RuntimeStreamerManager>,
    notification_service: Arc<NotificationService>,
    actor_registry: Arc<ActorRegistry>,
    /// Platform throttles, kept per route; an imported proxy that reaches
    /// another exit drops the old exit's.
    admission_backoff: Arc<crate::credentials::PlatformAdmission>,
    auth_import: Arc<dyn AuthImportTxPort>,
}

impl ConfigurationImportService {
    pub fn new(
        write_pool: SqlitePool,
        config_service: Arc<RuntimeConfigService>,
        streamer_manager: Arc<RuntimeStreamerManager>,
        notification_service: Arc<NotificationService>,
        actor_registry: Arc<ActorRegistry>,
        admission_backoff: Arc<crate::credentials::PlatformAdmission>,
        auth_import: Arc<dyn AuthImportTxPort>,
    ) -> Self {
        Self {
            write_pool,
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            config_service,
            streamer_manager,
            notification_service,
            actor_registry,
            admission_backoff,
            auth_import,
        }
    }

    pub async fn import_with_post_commit(
        &self,
        mut config: ConfigExport,
        mode: ImportMode,
        on_users_committed: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<ConfigurationImportOutcome, ConfigurationImportError> {
        validate_import(&config, mode)?;
        if includes_imported_users(&config) {
            self.auth_import.validate_user_exports(&config.users)?;
        }

        let state = self.streamer_manager.committed_state().ok_or_else(|| {
            ConfigurationImportError::from(crate::Error::Configuration(
                "Import requires the committed streamer state owner".to_owned(),
            ))
        })?;
        state.writer.require_serialized()?;
        state.writer.require_same_pool(&self.write_pool)?;
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| {
                ConfigurationImportError::from(crate::Error::Other(error.to_string()))
            })?;
        let cache = state.cache.clone();
        let config_service = self.config_service.clone();
        let auth_import = self.auth_import.clone();
        let service = self.clone();
        state
            .writer
            .transaction_with_completion(
                "configuration import",
                move |tx| {
                    Box::pin(async move {
                        let snapshot = ImportSnapshot::load(tx).await?;
                        snapshot.upgrade_bundle(&mut config, mode)?;
                        snapshot.validate_references(&config, mode)?;
                        auth_import
                            .validate_users_tx(
                                tx,
                                &config.users,
                                includes_imported_users(&config),
                                mode,
                            )
                            .await?;
                        let AppliedImport {
                            stats,
                            streamers,
                            moved_proxies,
                            kept_presets,
                        } = apply_import(tx, &snapshot, &config, mode, auth_import.as_ref())
                            .await?;
                        let retained_definitions =
                            sqlx::query_scalar("SELECT COUNT(*) FROM retirement_config_deletions")
                                .fetch_one(&mut *tx)
                                .await?;
                        let credential_retirement_streamers = sqlx::query_scalar("SELECT DISTINCT s.streamer_id FROM live_sessions s JOIN retirement_credential_profiles r ON json_extract(s.credential_binding, '$.identity.profile_id') = r.profile_id WHERE s.end_time IS NULL AND s.streamer_id IS NOT NULL")
                            .fetch_all(&mut *tx).await?;
                        Ok(CommittedImport {
                            stats,
                            streamers,
                            moved_proxies,
                            kept_presets,
                            retained_definitions,
                            credential_retirement_streamers,
                        })
                    })
                },
                move |committed| {
                    cache.apply(
                        &StateChange {
                            value: (),
                            rows: committed.streamers.rows.clone(),
                            removed: Vec::new(),
                            reconfigured: Vec::new(),
                        },
                        StatePublication::Silent,
                    );
                    config_service.invalidate_import_caches();
                    if let Some(callback) = on_users_committed {
                        callback();
                    }
                },
                move |committed| {
                    Box::pin(async move {
                        let _permit = permit;
                        service.finish_import(committed).await
                    })
                },
            )
            .await
    }

    async fn finish_import(
        &self,
        committed: CommittedImport,
    ) -> Result<ConfigurationImportOutcome, ConfigurationImportError> {
        let CommittedImport {
            stats,
            streamers,
            moved_proxies,
            kept_presets,
            retained_definitions,
            credential_retirement_streamers,
        } = committed;

        for id in moved_proxies {
            self.admission_backoff
                .clear_route(&crate::proxies::RouteKey::Proxy { id });
        }

        let mut warnings = kept_presets;
        if retained_definitions > 0 {
            warnings.push(format!(
                "{retained_definitions} omitted templates or presets remain available until recording post-processing finishes"
            ));
        }

        for streamer_id in &streamers.marked_deleted {
            self.streamer_manager.publish_imported(
                streamer_id,
                streamers.state_changed.contains(streamer_id),
                false,
            );
        }

        for streamer_id in &streamers.upserted {
            self.streamer_manager.publish_imported(
                streamer_id,
                streamers.state_changed.contains(streamer_id),
                true,
            );
        }

        self.config_service.notify_import_committed();

        if let Some(committed_state) = self.streamer_manager.committed_state() {
            let mut updated_streamers =
                Vec::with_capacity(streamers.marked_deleted.len() + streamers.upserted.len());
            updated_streamers.extend(streamers.marked_deleted.iter().cloned());
            updated_streamers.extend(streamers.upserted.iter().cloned());
            committed_state
                .notify_post_publication(crate::recorder::PostPublicationChangeSet {
                    updated_streamers,
                    removed_streamers: Vec::new(),
                    reconfigured_streamers: streamers.upserted.clone(),
                    outbox_enqueued: false,
                })
                .await;
        }

        for streamer_id in credential_retirement_streamers {
            self.actor_registry
                .settle_credential_retirement(&streamer_id)
                .await;
        }
        let reaped = async {
            let mut transaction = crate::database::begin_immediate(&self.write_pool).await?;
            crate::database::repositories::config_retirement::reap_retired_profiles(
                &mut transaction,
            )
            .await?;
            transaction.commit().await
        }
        .await;
        if let Err(error) = reaped {
            warnings.push(format!("credential retirement cleanup deferred: {error}"));
        }

        for streamer_id in &streamers.marked_deleted {
            let name = self
                .streamer_manager
                .get_streamer(streamer_id)
                .map(|metadata| metadata.name)
                .unwrap_or_default();
            if let Err(error) = self
                .actor_registry
                .retire_and_reap(streamer_id, &name, OBSERVE_RETIREMENT)
                .await
            {
                warnings.push(format!("streamer '{streamer_id}' removal failed: {error}"));
            }
        }

        if let Err(error) = self.notification_service.reload_from_db().await {
            warnings.push(format!("notification runtime reload failed: {error}"));
        }
        Ok(ConfigurationImportOutcome { stats, warnings })
    }
}

fn validation_error(message: impl Into<String>) -> ConfigurationImportError {
    ConfigurationImportError::Validation(message.into())
}

/// Database failures stay database errors; anything else rejects the bundle.
fn write_error(error: crate::Error) -> ConfigurationImportError {
    match error {
        crate::Error::DatabaseSqlx(error) => ConfigurationImportError::Database(error),
        crate::Error::Proxy(error @ crate::proxies::ProxyError::Missing(_)) => {
            ConfigurationImportError::MissingProxy(error)
        }
        error => validation_error(error.to_string()),
    }
}

fn validation<T>(message: impl Into<String>) -> Result<T, ConfigurationImportError> {
    Err(validation_error(message))
}

fn db_json(value: serde_json::Value) -> String {
    crate::config::backup::json_value_to_db_string(value)
}

#[derive(Default)]
struct ImportChanges {
    stats: ImportStats,
    streamers: StreamerImportDiff,
}

#[derive(Debug)]
struct AppliedImport {
    stats: ImportStats,
    streamers: StreamerImportDiff,
    moved_proxies: Vec<String>,
    kept_presets: Vec<String>,
}

async fn apply_import(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    mode: ImportMode,
    auth_import: &dyn AuthImportTxPort,
) -> Result<AppliedImport, ConfigurationImportError> {
    let replace = mode == ImportMode::Replace;
    let mut changes = ImportChanges::default();

    let imported_proxies = proxies::upsert(tx, config).await?;
    let config = &imported_proxies.store_step_routes(config)?;
    let global = global_model(&snapshot.global, config);
    persist_global(tx, &global).await?;
    imported_proxies
        .write_route(
            tx,
            &RouteOwner::Global,
            config.global_config.proxy_route.as_ref(),
            replace,
        )
        .await?;

    apply_engines(tx, snapshot, config, replace, &mut changes.stats).await?;
    let platform_ids: HashMap<String, String> = snapshot
        .platforms
        .iter()
        .map(|(name, model)| (name.clone(), model.id.clone()))
        .collect();
    profiles::upsert(tx, config, &platform_ids, &imported_proxies, replace).await?;
    let template_ids = apply_templates(
        tx,
        snapshot,
        config,
        replace,
        &imported_proxies,
        &mut changes.stats,
    )
    .await?;
    apply_platforms(
        tx,
        snapshot,
        config,
        replace,
        &imported_proxies,
        &mut changes.stats,
    )
    .await?;
    apply_streamers(
        tx,
        snapshot,
        config,
        replace,
        &ImportedScopes {
            templates: &template_ids,
            platforms: &platform_ids,
            proxies: &imported_proxies,
        },
        &mut changes,
    )
    .await?;
    delete_unimported_templates(tx, snapshot, config, replace, &mut changes.stats).await?;
    profiles::retire_omitted(tx, config, mode).await?;
    profiles::write_sites(tx, config, replace).await?;
    apply_notification_channels(
        tx,
        snapshot,
        config,
        replace,
        &imported_proxies,
        &mut changes.stats,
    )
    .await?;
    apply_job_presets(tx, snapshot, config, &mut changes.stats).await?;
    apply_pipeline_presets(tx, snapshot, config, &mut changes.stats).await?;
    let kept_presets = if replace {
        delete_omitted_presets(tx, snapshot, config, &mut changes.stats).await?
    } else {
        Vec::new()
    };
    proxies::delete_omitted(tx, config, mode).await?;
    auth_import
        .apply_auth_import_tx(
            tx,
            &config.users,
            includes_imported_users(config),
            mode,
            &mut changes.stats,
        )
        .await?;

    Ok(AppliedImport {
        stats: changes.stats,
        streamers: changes.streamers,
        moved_proxies: imported_proxies.moved,
        kept_presets,
    })
}
