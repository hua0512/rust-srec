use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use argon2::password_hash::PasswordHash;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use sqlx::SqlitePool;
use tracing::warn;

use crate::api::auth_service::MAX_USERNAME_LENGTH;
use crate::config::ConfigService;
use crate::config::backup::{
    ConfigExport, ImportMode, ImportStats, NotificationChannelExport, PipelinePresetExport,
};
use crate::credentials::{CredentialRefreshService, CredentialScope};
use crate::database::models::{
    ChannelType, EngineConfigurationDbModel, EngineType, FilterDbModel, FilterType,
    GlobalConfigDbModel, JobPreset, NotificationChannelDbModel, PipelinePreset,
    PlatformConfigDbModel, RetentionDays, StreamerDbModel, TemplateConfigDbModel, UserDbModel,
};
use crate::database::repositories::{
    config::SqlxConfigRepository, streamer::SqlxStreamerRepository,
};
use crate::database::{ImmediateTransaction, begin_immediate};
use crate::domain::StreamerState;
use crate::notification::NotificationService;
use crate::streamer::StreamerManager;

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamerManager = StreamerManager<SqlxStreamerRepository>;
type RuntimeCredentialService = CredentialRefreshService<SqlxConfigRepository>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigurationImportError {
    #[error("{0}")]
    Validation(String),
    #[error("configuration import database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    /// A streamer the bundle removes could not be taken off the runtime, so the
    /// import stops before its transaction deletes the row out from under a
    /// running download.
    #[error("configuration import could not stop a removed streamer: {0}")]
    StreamerStop(String),
}

#[derive(Debug)]
pub(crate) struct ConfigurationImportOutcome {
    pub stats: ImportStats,
    pub warnings: Vec<String>,
}

/// Runtime work an import must retire before its transaction removes a
/// `streamers` row.
///
/// Implemented by `RuntimeCoordinator::handle_streamer_removed`; the import
/// depends on the trait so tests can observe the call ordering without a
/// download runtime.
#[async_trait::async_trait]
pub(crate) trait StreamerRemovalStop: Send + Sync {
    /// Stop the streamer's queued and running downloads, its danmu collection
    /// and its open session, returning only once every download attempt has
    /// finalized.
    ///
    /// `Err` means the streamer may still be writing rows that the `streamers`
    /// delete would cascade away, and aborts the import; an attempt that simply
    /// finished on its own is not an error.
    async fn stop_for_removal(&self, streamer_id: &str) -> crate::Result<()>;
}

/// Streamer rows the import writes, keyed by what the runtime has to do about
/// them once the transaction commits.
#[derive(Debug, Default)]
struct StreamerImportDiff {
    /// Rows inserted or updated by `apply_streamers`; each is resynced through
    /// `StreamerManager::resync_streamer`.
    upserted: Vec<String>,
    /// Rows deleted by `apply_streamers` (replace mode only); each is evicted
    /// through `StreamerManager::evict_deleted_streamer`.
    deleted: Vec<String>,
}

/// Streamer resyncs in flight while `import` drains its post-commit diff. Bounds
/// the concurrent `StreamerManager::resync_streamer` calls, each of which hits
/// the streamer repository and publishes a config event.
const RESYNC_CONCURRENCY: usize = 8;

/// Upper bound on one [`StreamerRemovalStop::stop_for_removal`].
///
/// `DownloadManager::stop_download_with_reason` waits on the attempt's
/// completion without a deadline of its own, so an engine that never finishes
/// its graceful stop would hold the import's HTTP request open indefinitely.
/// Generous enough to cover an engine's default `graceful_stop_timeout_secs`
/// plus the danmu stop and the session end.
const REMOVAL_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

pub(crate) struct ConfigurationImportService {
    write_pool: SqlitePool,
    config_service: Arc<RuntimeConfigService>,
    streamer_manager: Arc<RuntimeStreamerManager>,
    notification_service: Arc<NotificationService>,
    credential_service: Arc<RuntimeCredentialService>,
    removal_stop: Arc<dyn StreamerRemovalStop>,
}

impl ConfigurationImportService {
    pub fn new(
        write_pool: SqlitePool,
        config_service: Arc<RuntimeConfigService>,
        streamer_manager: Arc<RuntimeStreamerManager>,
        notification_service: Arc<NotificationService>,
        credential_service: Arc<RuntimeCredentialService>,
        removal_stop: Arc<dyn StreamerRemovalStop>,
    ) -> Self {
        Self {
            write_pool,
            config_service,
            streamer_manager,
            notification_service,
            credential_service,
            removal_stop,
        }
    }

    pub async fn import(
        &self,
        config: ConfigExport,
        mode: ImportMode,
    ) -> Result<ConfigurationImportOutcome, ConfigurationImportError> {
        validate_import(&config, mode)?;

        // Resolve the bundle against the current rows before touching the
        // runtime: `stop_for_removal` finalizes downloads and ends sessions, and
        // nothing undoes that if the import is rejected afterwards. The
        // connection is scoped so it returns to the single-connection write pool
        // before `begin_immediate` asks for one.
        let removed_streamer_ids = {
            let mut conn = self.write_pool.acquire().await?;
            let preflight = ImportSnapshot::load(&mut conn).await?;
            preflight.validate_references(&config, mode)?;
            streamers_removed_by(&preflight.streamers, &config, mode)
        };

        // Retire the runtime work of every row the import is about to delete
        // before opening the write transaction: `streamers.id` cascades into
        // `live_sessions`, `media_outputs` and `session_segments`, so an attempt
        // still writing segments would fail its foreign key mid-recording. The
        // stops are independent per streamer and each can wait out an engine's
        // graceful-stop timeout, so they run together under one bound.
        for result in futures::future::join_all(removed_streamer_ids.iter().map(|streamer_id| {
            tokio::time::timeout(
                REMOVAL_STOP_TIMEOUT,
                self.removal_stop.stop_for_removal(streamer_id),
            )
        }))
        .await
        {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(ConfigurationImportError::StreamerStop(error.to_string()));
                }
                Err(_) => {
                    return Err(ConfigurationImportError::StreamerStop(format!(
                        "a removed streamer did not release its recording within {} seconds",
                        REMOVAL_STOP_TIMEOUT.as_secs()
                    )));
                }
            }
        }

        let mut tx = begin_immediate(&self.write_pool).await?;
        let snapshot = ImportSnapshot::load(&mut tx).await?;
        snapshot.validate_references(&config, mode)?;

        // The stop phase runs unlocked, so a streamer can be created or have its
        // URL changed between the preflight snapshot and this transaction.
        // `streamers_removed_by` and the delete loop in `apply_streamers` share
        // the `streamer_for_update` retention rule, so this recomputation names
        // exactly the rows `apply_streamers` would delete; anything not already
        // stopped aborts the import and rolls the transaction back.
        for streamer_id in streamers_removed_by(&snapshot.streamers, &config, mode) {
            if !removed_streamer_ids.contains(&streamer_id) {
                return Err(ConfigurationImportError::StreamerStop(format!(
                    "streamer {streamer_id} appeared or changed while the import was stopping recordings; retry"
                )));
            }
        }

        let (stats, invalidated_credentials, streamer_diff) =
            apply_import(&mut tx, &snapshot, &config, mode).await?;
        tx.commit().await?;

        for scope in invalidated_credentials {
            self.credential_service.invalidate(&scope);
        }

        let mut warnings = Vec::new();
        for streamer_id in &streamer_diff.deleted {
            self.streamer_manager.evict_deleted_streamer(streamer_id);
        }

        // Bring the in-memory row and the scheduler's actor in line with what was
        // just committed. `resync_streamer` publishes
        // `ConfigUpdateEvent::StreamerMetadataUpdated`, which spawns an actor for a
        // newly imported streamer and tears one down for a streamer the bundle
        // disabled.
        let mut resyncs = Vec::with_capacity(streamer_diff.upserted.len());
        for streamer_id in streamer_diff.upserted {
            let streamer_manager = Arc::clone(&self.streamer_manager);
            resyncs.push(async move {
                streamer_manager
                    .resync_streamer(&streamer_id)
                    .await
                    .err()
                    .map(|error| format!("streamer '{streamer_id}' runtime reload failed: {error}"))
            });
        }
        let mut resyncs = stream::iter(resyncs).buffer_unordered(RESYNC_CONCURRENCY);
        while let Some(warning) = resyncs.next().await {
            warnings.extend(warning);
        }

        if let Err(error) = self.notification_service.reload_from_db().await {
            warnings.push(format!("notification runtime reload failed: {error}"));
        }
        self.config_service.notify_import_committed();

        Ok(ConfigurationImportOutcome { stats, warnings })
    }
}

struct ImportSnapshot {
    global: GlobalConfigDbModel,
    engines: HashMap<String, EngineConfigurationDbModel>,
    templates: HashMap<String, TemplateConfigDbModel>,
    platforms: HashMap<String, PlatformConfigDbModel>,
    // Shaped by load_streamer_rows: streamer_for_update picks the row an import
    // writes to, and apply_streamers deletes (Replace) or warns about (Merge) the
    // rest of each case-insensitive group.
    streamers: HashMap<String, Vec<StreamerDbModel>>,
    channels: HashMap<String, NotificationChannelDbModel>,
    job_presets: HashMap<String, JobPreset>,
    pipeline_presets: HashMap<String, PipelinePreset>,
    users: HashMap<String, UserDbModel>,
}

impl ImportSnapshot {
    /// Reads over `conn` rather than the write transaction so
    /// `ConfigurationImportService::import` can validate the bundle against the
    /// current rows before `begin_immediate`.
    async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let global = sqlx::query_as::<_, GlobalConfigDbModel>(
            "SELECT * FROM global_config ORDER BY rowid LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await?;
        let engines =
            sqlx::query_as::<_, EngineConfigurationDbModel>("SELECT * FROM engine_configuration")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();
        let templates = sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), model))
            .collect();
        let platforms = sqlx::query_as::<_, PlatformConfigDbModel>("SELECT * FROM platform_config")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|model| (model.platform_name.clone(), model))
            .collect();
        let streamers = load_streamer_rows(&mut *conn).await?;
        let channels =
            sqlx::query_as::<_, NotificationChannelDbModel>("SELECT * FROM notification_channel")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();
        let job_presets = sqlx::query_as::<_, JobPreset>("SELECT * FROM job_presets")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), model))
            .collect();
        let pipeline_presets =
            sqlx::query_as::<_, PipelinePreset>("SELECT * FROM pipeline_presets")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();
        let users = sqlx::query_as::<_, UserDbModel>("SELECT * FROM users")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|model| (model.username.clone(), model))
            .collect();

        Ok(Self {
            global,
            engines,
            templates,
            platforms,
            streamers,
            channels,
            job_presets,
            pipeline_presets,
            users,
        })
    }

    fn streamer_for_update(&self, url: &str) -> Option<&StreamerDbModel> {
        streamer_for_update(&self.streamers, url)
    }

    fn validate_references(
        &self,
        config: &ConfigExport,
        mode: ImportMode,
    ) -> Result<(), ConfigurationImportError> {
        for platform in &config.platforms {
            if !self.platforms.contains_key(&platform.platform_name) {
                return validation(format!(
                    "Unknown platform '{}' in import bundle",
                    platform.platform_name
                ));
            }
        }

        let mut template_names: HashSet<&str> = config
            .templates
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        if mode == ImportMode::Merge {
            template_names.extend(self.templates.keys().map(String::as_str));
        }

        let mut engine_names: HashSet<&str> = config
            .engines
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        if mode == ImportMode::Merge {
            engine_names.extend(self.engines.keys().map(String::as_str));
        }
        validate_engine_reference(
            "global_config.default_download_engine",
            Some(config.global_config.default_download_engine.as_str()),
            &engine_names,
        )?;

        for template in &config.templates {
            validate_engine_reference(
                &format!("template '{}'.download_engine", template.name),
                template.download_engine.as_deref(),
                &engine_names,
            )?;
        }
        for platform in &config.platforms {
            validate_engine_reference(
                &format!("platform '{}'.download_engine", platform.platform_name),
                platform.download_engine.as_deref(),
                &engine_names,
            )?;
        }

        for streamer in &config.streamers {
            if !self.platforms.contains_key(&streamer.platform) {
                return validation(format!(
                    "Unknown platform '{}' for streamer '{}'",
                    streamer.platform, streamer.name
                ));
            }
            if let Some(template) = streamer.template.as_deref()
                && !template_names.contains(template)
            {
                return validation(format!(
                    "Unknown template '{}' for streamer '{}'",
                    template, streamer.name
                ));
            }
        }

        let existing_ids: HashMap<&str, &str> = self
            .users
            .values()
            .map(|user| (user.id.as_str(), user.username.as_str()))
            .collect();
        for user in &config.users {
            if self.users.contains_key(&user.username) {
                continue;
            }
            if let Some(existing_username) = existing_ids.get(user.id.as_str()) {
                return validation(format!(
                    "User id '{}' is already in use by username '{}'",
                    user.id, existing_username
                ));
            }
        }

        Ok(())
    }
}

/// Every `streamers` row grouped by `url.to_ascii_lowercase()`, each group sorted
/// by id.
///
/// The schema declares `streamers.url COLLATE NOCASE UNIQUE`, but databases whose
/// streamers table predates that collation enforce case-sensitive uniqueness only,
/// so one key can hold several rows. Lowest id first: [`streamer_for_update`]
/// falls back to `rows[0]` when no byte-equal URL match exists, and that pick must
/// not depend on row fetch order.
async fn load_streamer_rows<'e, E>(
    executor: E,
) -> Result<HashMap<String, Vec<StreamerDbModel>>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let mut streamers: HashMap<String, Vec<StreamerDbModel>> = HashMap::new();
    for model in sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers")
        .fetch_all(executor)
        .await?
    {
        streamers
            .entry(model.url.to_ascii_lowercase())
            .or_default()
            .push(model);
    }
    for rows in streamers.values_mut() {
        rows.sort_by(|a, b| a.id.cmp(&b.id));
    }
    Ok(streamers)
}

/// Row that receives imported updates for `url`: the byte-equal URL match when one
/// exists, otherwise the lowest-id row of the case-insensitive group. Preferring
/// the byte-equal match matters on tables without the NOCASE collation:
/// persist_streamer rewrites streamers.url, and writing one row's exact URL onto a
/// sibling id would collide on the url UNIQUE constraint while the shadowed row
/// still exists.
fn streamer_for_update<'a>(
    streamers: &'a HashMap<String, Vec<StreamerDbModel>>,
    url: &str,
) -> Option<&'a StreamerDbModel> {
    let rows = streamers.get(&url.to_ascii_lowercase())?;
    rows.iter()
        .find(|row| row.url == url)
        .or_else(|| rows.first())
}

/// Ids of `streamers` rows `apply_streamers` will delete: in replace mode every
/// row no bundle entry routes to, which covers unmatched URLs and the
/// case-duplicate rows [`streamer_for_update`] passes over. Empty in merge mode,
/// which never deletes streamer rows.
fn streamers_removed_by(
    streamers: &HashMap<String, Vec<StreamerDbModel>>,
    config: &ConfigExport,
    mode: ImportMode,
) -> Vec<String> {
    if mode != ImportMode::Replace {
        return Vec::new();
    }
    let retained: HashSet<&str> = config
        .streamers
        .iter()
        .filter_map(|item| streamer_for_update(streamers, &item.url))
        .map(|row| row.id.as_str())
        .collect();
    streamers
        .values()
        .flatten()
        .filter(|row| !retained.contains(row.id.as_str()))
        .map(|row| row.id.clone())
        .collect()
}

fn validation_error(message: impl Into<String>) -> ConfigurationImportError {
    ConfigurationImportError::Validation(message.into())
}

fn validation<T>(message: impl Into<String>) -> Result<T, ConfigurationImportError> {
    Err(validation_error(message))
}

fn validate_import(
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    if !config.version.starts_with("0.") {
        return validation(format!(
            "Unsupported schema version: {}. Expected 0.x",
            config.version
        ));
    }
    RetentionDays::try_from(config.global_config.job_history_retention_days)
        .map_err(|error| validation_error(error.to_string()))?;
    RetentionDays::try_from(config.global_config.notification_event_log_retention_days)
        .map_err(|error| validation_error(error.to_string()))?;

    validate_non_empty("global output_folder", &config.global_config.output_folder)?;
    validate_non_empty(
        "global output_filename_template",
        &config.global_config.output_filename_template,
    )?;
    validate_non_empty(
        "global default_download_engine",
        &config.global_config.default_download_engine,
    )?;
    if config.global_config.max_concurrent_downloads < 1 {
        return validation("max_concurrent_downloads must be at least 1");
    }
    if config.global_config.max_concurrent_uploads < 1 {
        return validation("max_concurrent_uploads must be at least 1");
    }
    if config.global_config.offline_check_count < 1 {
        return validation("offline_check_count must be at least 1");
    }
    if config.global_config.streamer_check_delay_ms < 1
        || config.global_config.offline_check_delay_ms < 1
    {
        return validation("streamer and offline check delays must be positive");
    }

    validate_pipeline_values(
        "global config",
        [
            config.global_config.pipeline.as_ref(),
            config.global_config.session_complete_pipeline.as_ref(),
            config.global_config.paired_segment_pipeline.as_ref(),
        ],
    )?;

    validate_unique(
        "engine name",
        config.engines.iter().map(|item| item.name.as_str()),
        false,
    )?;
    for engine in &config.engines {
        validate_non_empty("engine name", &engine.name)?;
        if EngineType::parse(&engine.engine_type).is_none() {
            return validation(format!(
                "Invalid engine type '{}' for engine '{}'",
                engine.engine_type, engine.name
            ));
        }
    }

    validate_unique(
        "template name",
        config.templates.iter().map(|item| item.name.as_str()),
        false,
    )?;
    for template in &config.templates {
        validate_non_empty("template name", &template.name)?;
        validate_pipeline_values(
            &format!("template '{}'", template.name),
            [
                template.pipeline.as_ref(),
                template.session_complete_pipeline.as_ref(),
                template.paired_segment_pipeline.as_ref(),
            ],
        )?;
    }

    validate_unique(
        "platform name",
        config
            .platforms
            .iter()
            .map(|item| item.platform_name.as_str()),
        false,
    )?;
    for platform in &config.platforms {
        validate_non_empty("platform name", &platform.platform_name)?;
        validate_pipeline_values(
            &format!("platform '{}'", platform.platform_name),
            [
                platform.pipeline.as_ref(),
                platform.session_complete_pipeline.as_ref(),
                platform.paired_segment_pipeline.as_ref(),
            ],
        )?;
    }

    validate_unique(
        "streamer URL",
        config.streamers.iter().map(|item| item.url.as_str()),
        true,
    )?;
    for streamer in &config.streamers {
        validate_non_empty("streamer name", &streamer.name)?;
        validate_non_empty("streamer URL", &streamer.url)?;
        if crate::domain::StreamerState::parse(&streamer.state).is_none() {
            return validation(format!(
                "Invalid state '{}' for streamer '{}'",
                streamer.state, streamer.name
            ));
        }
        if crate::domain::Priority::parse(&streamer.priority).is_none() {
            return validation(format!(
                "Invalid priority '{}' for streamer '{}'",
                streamer.priority, streamer.name
            ));
        }
        for filter in &streamer.filters {
            let Some(filter_type) = FilterType::parse(&filter.filter_type) else {
                return validation(format!(
                    "Invalid filter type '{}' for streamer '{}'",
                    filter.filter_type, streamer.name
                ));
            };
            let model = FilterDbModel::new(
                "validation",
                filter_type,
                crate::config::backup::json_value_to_db_string(filter.config.clone()),
            );
            crate::domain::filter::Filter::try_from(&model).map_err(|error| {
                validation_error(format!(
                    "Invalid {} filter for streamer '{}': {}",
                    filter.filter_type, streamer.name, error
                ))
            })?;
        }
    }

    validate_unique(
        "notification channel name",
        config
            .notification_channels
            .iter()
            .map(|item| item.name.as_str()),
        false,
    )?;
    for channel in &config.notification_channels {
        validate_notification_channel(channel)?;
    }

    validate_unique(
        "job preset name",
        config.job_presets.iter().map(|item| item.name.as_str()),
        false,
    )?;
    for preset in &config.job_presets {
        let mut model = JobPreset::new(&preset.name, &preset.processor, preset.config.clone());
        model.description = preset.description.clone();
        model.category = preset.category.clone();
        model.validate().map_err(|error| {
            validation_error(format!("Invalid job preset '{}': {}", preset.name, error))
        })?;
    }

    validate_unique(
        "pipeline preset name",
        config
            .pipeline_presets
            .iter()
            .map(|item| item.name.as_str()),
        false,
    )?;
    for preset in &config.pipeline_presets {
        validate_pipeline_preset(preset)?;
    }

    let includes_users = crate::config::backup::schema_version_at_least(&config.version, (0, 1, 3))
        && !config.users.is_empty();
    if includes_users {
        validate_unique(
            "username",
            config.users.iter().map(|item| item.username.as_str()),
            false,
        )?;
        validate_unique(
            "user id",
            config.users.iter().map(|item| item.id.as_str()),
            false,
        )?;
        validate_unique(
            "user email",
            config.users.iter().filter_map(|item| item.email.as_deref()),
            true,
        )?;
        if mode == ImportMode::Replace
            && !config
                .users
                .iter()
                .any(|user| user.is_active && user.roles.iter().any(|role| role == "admin"))
        {
            return validation(
                "Replace import requires at least one active user with the 'admin' role",
            );
        }
        for user in &config.users {
            validate_non_empty("user id", &user.id)?;
            validate_non_empty("username", &user.username)?;
            // The same bound `AuthService::authenticate` applies, so an
            // import cannot create a user who is then refused at login.
            if user.username.chars().count() > MAX_USERNAME_LENGTH {
                return validation(format!(
                    "Username '{}' exceeds {} characters",
                    user.username, MAX_USERNAME_LENGTH
                ));
            }
            if user.roles.is_empty() || user.roles.iter().any(|role| role.trim().is_empty()) {
                return validation(format!(
                    "User '{}' must have at least one non-empty role",
                    user.username
                ));
            }
            let parsed = PasswordHash::new(&user.password_hash).map_err(|error| {
                validation_error(format!(
                    "Invalid password hash for user '{}': {}",
                    user.username, error
                ))
            })?;
            if parsed.algorithm.as_str() != "argon2id" {
                return validation(format!(
                    "Password hash for user '{}' must use Argon2id",
                    user.username
                ));
            }
        }
    }

    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), ConfigurationImportError> {
    if value.trim().is_empty() {
        return validation(format!("{label} cannot be empty"));
    }
    Ok(())
}

fn validate_unique<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a str>,
    case_insensitive: bool,
) -> Result<(), ConfigurationImportError> {
    let mut seen = HashSet::new();
    for value in values {
        let key = if case_insensitive {
            value.to_ascii_lowercase()
        } else {
            value.to_string()
        };
        if !seen.insert(key) {
            return validation(format!("Duplicate {label} '{value}'"));
        }
    }
    Ok(())
}

fn validate_pipeline_values<'a>(
    owner: &str,
    values: impl IntoIterator<Item = Option<&'a serde_json::Value>>,
) -> Result<(), ConfigurationImportError> {
    for value in values.into_iter().flatten() {
        let normalized = crate::config::backup::unwrap_json_value(value.clone());
        let dag: crate::database::models::job::DagPipelineDefinition =
            serde_json::from_value(normalized).map_err(|error| {
                validation_error(format!("Invalid pipeline definition in {owner}: {error}"))
            })?;
        dag.validate().map_err(|error| {
            validation_error(format!("Invalid pipeline definition in {owner}: {error}"))
        })?;
    }
    Ok(())
}

fn validate_pipeline_preset(preset: &PipelinePresetExport) -> Result<(), ConfigurationImportError> {
    let Some(value) = preset.dag_definition.clone() else {
        return validation(format!(
            "Missing dag_definition for pipeline preset '{}'",
            preset.name
        ));
    };
    let normalized = crate::config::backup::unwrap_json_value(value);
    let dag = serde_json::from_value(normalized).map_err(|error| {
        validation_error(format!(
            "Invalid dag_definition for pipeline preset '{}': {}",
            preset.name, error
        ))
    })?;
    let mut model = PipelinePreset::new(&preset.name, dag);
    model.description = preset.description.clone();
    model.pipeline_type = preset.pipeline_type.clone();
    model.validate().map_err(|error| {
        validation_error(format!(
            "Invalid pipeline preset '{}': {}",
            preset.name, error
        ))
    })
}

fn validate_notification_channel(
    channel: &NotificationChannelExport,
) -> Result<(), ConfigurationImportError> {
    validate_non_empty("notification channel name", &channel.name)?;
    let Some(channel_type) = ChannelType::parse(&channel.channel_type) else {
        return validation(format!(
            "Invalid notification channel type '{}' for '{}'",
            channel.channel_type, channel.name
        ));
    };
    let settings = crate::config::backup::unwrap_json_value(channel.settings.clone());
    let parse_result = match channel_type {
        ChannelType::Discord => {
            serde_json::from_value::<crate::database::models::DiscordChannelSettings>(settings)
                .map(|_| ())
        }
        ChannelType::Email => {
            serde_json::from_value::<crate::database::models::EmailChannelSettings>(settings)
                .map(|_| ())
        }
        ChannelType::Gotify => {
            serde_json::from_value::<crate::database::models::GotifyChannelSettings>(settings)
                .map(|_| ())
        }
        ChannelType::Telegram => {
            serde_json::from_value::<crate::database::models::TelegramChannelSettings>(settings)
                .map(|_| ())
        }
        ChannelType::Webhook => {
            serde_json::from_value::<crate::database::models::WebhookChannelSettings>(settings)
                .map(|_| ())
        }
    };
    parse_result.map_err(|error| {
        validation_error(format!(
            "Invalid settings for notification channel '{}': {}",
            channel.name, error
        ))
    })?;
    validate_unique(
        &format!("subscription for channel '{}'", channel.name),
        channel.subscriptions.iter().map(String::as_str),
        false,
    )
}

fn validate_engine_reference(
    label: &str,
    value: Option<&str>,
    engine_names: &HashSet<&str>,
) -> Result<(), ConfigurationImportError> {
    if let Some(name) = value
        && !name.trim().is_empty()
        && !engine_names.contains(name)
    {
        return validation(format!("Unknown engine '{name}' referenced by {label}"));
    }
    Ok(())
}

/// Mutable outcome threaded through the credential-bearing apply_* passes:
/// the per-entity counters apply_import returns to the caller, the
/// `CredentialScope`s that `ConfigurationImportService::import` feeds to
/// `credential_service.invalidate` after the transaction commits, and the
/// streamer ids it then pushes through `StreamerManager`.
#[derive(Default)]
struct ImportChanges {
    stats: ImportStats,
    invalidated_credentials: Vec<CredentialScope>,
    streamers: StreamerImportDiff,
}

async fn apply_import(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(ImportStats, Vec<CredentialScope>, StreamerImportDiff), ConfigurationImportError> {
    let replace = mode == ImportMode::Replace;
    let mut changes = ImportChanges::default();

    let global = global_model(&snapshot.global, config);
    persist_global(tx, &global).await?;

    apply_engines(tx, snapshot, config, replace, &mut changes.stats).await?;
    let template_ids = apply_templates(tx, snapshot, config, replace, &mut changes).await?;
    let platform_ids = apply_platforms(tx, snapshot, config, &mut changes).await?;
    apply_streamers(
        tx,
        snapshot,
        config,
        replace,
        &template_ids,
        &platform_ids,
        &mut changes,
    )
    .await?;
    delete_unimported_templates(tx, snapshot, config, replace, &mut changes).await?;

    apply_notification_channels(tx, snapshot, config, replace, &mut changes.stats).await?;
    apply_job_presets(tx, snapshot, config, replace, &mut changes.stats).await?;
    apply_pipeline_presets(tx, snapshot, config, replace, &mut changes.stats).await?;
    apply_users(tx, snapshot, config, replace, &mut changes.stats).await?;

    sqlx::query("DELETE FROM refresh_tokens")
        .execute(&mut **tx)
        .await?;

    Ok((
        changes.stats,
        changes.invalidated_credentials,
        changes.streamers,
    ))
}

async fn apply_engines(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.engines {
        let model = if let Some(existing) = snapshot.engines.get(&item.name) {
            stats.engines_updated += 1;
            EngineConfigurationDbModel {
                id: existing.id.clone(),
                name: item.name.clone(),
                engine_type: item.engine_type.clone(),
                config: db_json(item.config.clone()),
            }
        } else {
            stats.engines_created += 1;
            EngineConfigurationDbModel::new(
                &item.name,
                EngineType::parse(&item.engine_type).ok_or_else(|| {
                    validation_error(format!("Invalid engine type '{}'", item.engine_type))
                })?,
                db_json(item.config.clone()),
            )
        };
        persist_engine(tx, &model).await?;
    }
    if replace {
        let imported: HashSet<&str> = config
            .engines
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.engines {
            if !imported.contains(name.as_str()) {
                sqlx::query("DELETE FROM engine_configuration WHERE id = ?")
                    .bind(&existing.id)
                    .execute(&mut **tx)
                    .await?;
                stats.engines_deleted += 1;
            }
        }
    }
    Ok(())
}

/// Upserts bundle templates and returns name -> template_config.id for
/// apply_streamers to resolve streamer template references. Replace mode may
/// only reference bundle templates, so the map starts empty; merge seeds it
/// from the snapshot so streamers can keep pointing at templates the bundle
/// does not carry. Replace-mode deletion of snapshot-only templates is
/// deferred to delete_unimported_templates, which must run after
/// apply_streamers.
async fn apply_templates(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    changes: &mut ImportChanges,
) -> Result<HashMap<String, String>, ConfigurationImportError> {
    let mut template_ids: HashMap<String, String> = if replace {
        HashMap::new()
    } else {
        snapshot
            .templates
            .iter()
            .map(|(name, model)| (name.clone(), model.id.clone()))
            .collect()
    };
    for item in &config.templates {
        let existing = snapshot.templates.get(&item.name);
        let model = template_model(existing, item);
        persist_template(tx, &model).await?;
        template_ids.insert(model.name.clone(), model.id.clone());
        changes
            .invalidated_credentials
            .push(CredentialScope::Template {
                template_id: model.id.clone(),
                template_name: model.name.clone(),
            });
        if existing.is_some() {
            changes.stats.templates_updated += 1;
        } else {
            changes.stats.templates_created += 1;
        }
    }
    Ok(template_ids)
}

/// Updates the platform_config rows named by the bundle and returns
/// platform_name -> platform_config.id for apply_streamers. Imports never
/// create or delete platform rows; validate_references already rejected
/// bundle platforms absent from the snapshot.
async fn apply_platforms(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    changes: &mut ImportChanges,
) -> Result<HashMap<String, String>, ConfigurationImportError> {
    let mut platform_ids: HashMap<String, String> = snapshot
        .platforms
        .iter()
        .map(|(name, model)| (name.clone(), model.id.clone()))
        .collect();
    for item in &config.platforms {
        let existing = snapshot.platforms.get(&item.platform_name).ok_or_else(|| {
            validation_error(format!("Unknown platform '{}'", item.platform_name))
        })?;
        let model = platform_model(existing, item);
        persist_platform(tx, &model).await?;
        platform_ids.insert(model.platform_name.clone(), model.id.clone());
        changes
            .invalidated_credentials
            .push(CredentialScope::Platform {
                platform_id: model.id.clone(),
                platform_name: model.platform_name.clone(),
            });
        changes.stats.platforms_updated += 1;
    }
    Ok(platform_ids)
}

async fn apply_streamers(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    template_ids: &HashMap<String, String>,
    platform_ids: &HashMap<String, String>,
    changes: &mut ImportChanges,
) -> Result<(), ConfigurationImportError> {
    // Ids written by the streamer loop below (updated or newly created). Replace
    // mode deletes every snapshot row absent from this set, which covers both
    // unmatched URLs and case-duplicate rows that streamer_for_update passed over.
    let mut retained_streamer_ids: HashSet<String> = HashSet::new();
    for item in &config.streamers {
        let existing = snapshot.streamer_for_update(&item.url);
        let platform_id = platform_ids.get(&item.platform).ok_or_else(|| {
            validation_error(format!(
                "Unknown platform '{}' for streamer '{}'",
                item.platform, item.name
            ))
        })?;
        let template_id = item
            .template
            .as_ref()
            .map(|name| {
                template_ids.get(name).cloned().ok_or_else(|| {
                    validation_error(format!(
                        "Unknown template '{}' for streamer '{}'",
                        name, item.name
                    ))
                })
            })
            .transpose()?;
        let model = streamer_model(existing, item, platform_id, template_id);
        persist_streamer(tx, &model).await?;
        sqlx::query("DELETE FROM filters WHERE streamer_id = ?")
            .bind(&model.id)
            .execute(&mut **tx)
            .await?;
        for item_filter in &item.filters {
            let filter_type = FilterType::parse(&item_filter.filter_type).ok_or_else(|| {
                validation_error(format!("Invalid filter type '{}'", item_filter.filter_type))
            })?;
            let filter =
                FilterDbModel::new(&model.id, filter_type, db_json(item_filter.config.clone()));
            persist_filter(tx, &filter).await?;
        }
        changes
            .invalidated_credentials
            .push(CredentialScope::Streamer {
                streamer_id: model.id.clone(),
                streamer_name: model.name.clone(),
            });
        retained_streamer_ids.insert(model.id.clone());
        changes.streamers.upserted.push(model.id.clone());
        if existing.is_some() {
            changes.stats.streamers_updated += 1;
        } else {
            changes.stats.streamers_created += 1;
        }
    }
    if !replace {
        // Merge mode never deletes streamer rows, so every row of a case-duplicate
        // group survives even though streamer_for_update routes an imported URL to
        // a single row; surface the group so operators can remove the shadowed rows.
        for rows in snapshot.streamers.values() {
            if rows.len() > 1 {
                let colliding: Vec<&str> = rows.iter().map(|row| row.url.as_str()).collect();
                warn!(
                    urls = %colliding.join(", "),
                    "Streamer rows share a URL differing only by case; merge import updates at most one of them and keeps the rest"
                );
            }
        }
    }
    if replace {
        for rows in snapshot.streamers.values() {
            for existing in rows {
                if retained_streamer_ids.contains(&existing.id) {
                    continue;
                }
                sqlx::query("DELETE FROM streamers WHERE id = ?")
                    .bind(&existing.id)
                    .execute(&mut **tx)
                    .await?;
                changes
                    .invalidated_credentials
                    .push(CredentialScope::Streamer {
                        streamer_id: existing.id.clone(),
                        streamer_name: existing.name.clone(),
                    });
                changes.streamers.deleted.push(existing.id.clone());
                changes.stats.streamers_deleted += 1;
            }
        }
    }
    Ok(())
}

/// Replace-mode deletion of snapshot templates absent from the bundle; no-op
/// in merge mode. Must run after apply_streamers: until persist_streamer
/// rewrites each retained row's template_config_id (Replace-mode template_ids
/// resolves bundle templates only) and the unretained rows are deleted,
/// snapshot streamer rows may still reference the template_config rows
/// removed here.
async fn delete_unimported_templates(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    changes: &mut ImportChanges,
) -> Result<(), ConfigurationImportError> {
    if !replace {
        return Ok(());
    }
    let imported_templates: HashSet<&str> = config
        .templates
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    for (name, existing) in &snapshot.templates {
        if !imported_templates.contains(name.as_str()) {
            sqlx::query("DELETE FROM template_config WHERE id = ?")
                .bind(&existing.id)
                .execute(&mut **tx)
                .await?;
            changes
                .invalidated_credentials
                .push(CredentialScope::Template {
                    template_id: existing.id.clone(),
                    template_name: existing.name.clone(),
                });
            changes.stats.templates_deleted += 1;
        }
    }
    Ok(())
}

fn global_model(existing: &GlobalConfigDbModel, config: &ConfigExport) -> GlobalConfigDbModel {
    let source = &config.global_config;
    let mut model = existing.clone();
    model.output_folder = source.output_folder.clone();
    model.output_filename_template = source.output_filename_template.clone();
    model.output_file_format = source.output_file_format.clone();
    model.min_segment_size_bytes = source.min_segment_size_bytes;
    model.max_download_duration_secs = source.max_download_duration_secs;
    model.max_part_size_bytes = source.max_part_size_bytes;
    model.record_danmu = source.record_danmu;
    model.danmu_statistics = source.danmu_statistics.clone();
    model.max_concurrent_downloads = source.max_concurrent_downloads;
    model.max_concurrent_uploads = source.max_concurrent_uploads;
    model.streamer_check_delay_ms = source.streamer_check_delay_ms;
    model.proxy_config = db_json(source.proxy_config.clone());
    model.offline_check_delay_ms = source.offline_check_delay_ms;
    model.offline_check_count = source.offline_check_count;
    model.default_download_engine = source.default_download_engine.clone();
    model.max_concurrent_cpu_jobs = source.max_concurrent_cpu_jobs;
    model.max_concurrent_io_jobs = source.max_concurrent_io_jobs;
    model.job_history_retention_days = source.job_history_retention_days;
    model.notification_event_log_retention_days = source.notification_event_log_retention_days;
    model.pipeline = source.pipeline.clone().map(db_json);
    model.session_complete_pipeline = source.session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = source.paired_segment_pipeline.clone().map(db_json);
    if let Some(log_filter) = &source.log_filter_directive {
        model.log_filter_directive = log_filter.clone();
    }
    model.auto_thumbnail = source.auto_thumbnail;
    model.pipeline_cpu_job_timeout_secs = source.pipeline_cpu_job_timeout_secs;
    model.pipeline_io_job_timeout_secs = source.pipeline_io_job_timeout_secs;
    model.pipeline_execute_timeout_secs = source.pipeline_execute_timeout_secs;
    model.queue_freshness_threshold_ms = source.queue_freshness_threshold_ms;
    model.gpu_health_probe_interval_secs = source.gpu_health_probe_interval_secs;
    model.stream_proxy_allow_private_targets = source.stream_proxy_allow_private_targets;
    model
}

fn template_model(
    existing: Option<&TemplateConfigDbModel>,
    source: &crate::config::backup::TemplateExport,
) -> TemplateConfigDbModel {
    let mut model = existing
        .cloned()
        .unwrap_or_else(|| TemplateConfigDbModel::new(&source.name));
    model.name = source.name.clone();
    model.output_folder = source.output_folder.clone();
    model.output_filename_template = source.output_filename_template.clone();
    model.cookies = source.cookies.clone();
    model.output_file_format = source.output_file_format.clone();
    model.min_segment_size_bytes = source.min_segment_size_bytes;
    model.max_download_duration_secs = source.max_download_duration_secs;
    model.max_part_size_bytes = source.max_part_size_bytes;
    model.record_danmu = source.record_danmu;
    model.danmu_statistics = source.danmu_statistics.clone();
    model.platform_overrides = source.platform_overrides.clone().map(db_json);
    model.download_retry_policy = source.download_retry_policy.clone().map(db_json);
    model.download_engine = source.download_engine.clone();
    model.engines_override = source.engines_override.clone().map(db_json);
    model.proxy_config = source.proxy_config.clone().map(db_json);
    model.stream_selection_config = source.stream_selection_config.clone().map(db_json);
    model.pipeline = source.pipeline.clone().map(db_json);
    model.session_complete_pipeline = source.session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = source.paired_segment_pipeline.clone().map(db_json);
    model.offline_check_count = source.offline_check_count;
    model.offline_check_delay_ms = source.offline_check_delay_ms;
    model.updated_at = Utc::now();
    model
}

fn platform_model(
    existing: &PlatformConfigDbModel,
    source: &crate::config::backup::PlatformExport,
) -> PlatformConfigDbModel {
    let mut model = existing.clone();
    model.fetch_delay_ms = source.fetch_delay_ms;
    model.download_delay_ms = source.download_delay_ms;
    model.cookies = source.cookies.clone();
    model.platform_specific_config = source.platform_specific_config.clone().map(db_json);
    model.proxy_config = source.proxy_config.clone().map(db_json);
    model.record_danmu = source.record_danmu;
    model.danmu_statistics = source.danmu_statistics.clone();
    model.output_folder = source.output_folder.clone();
    model.output_filename_template = source.output_filename_template.clone();
    model.download_engine = source.download_engine.clone();
    model.stream_selection_config = source.stream_selection_config.clone().map(db_json);
    model.output_file_format = source.output_file_format.clone();
    model.min_segment_size_bytes = source.min_segment_size_bytes;
    model.max_download_duration_secs = source.max_download_duration_secs;
    model.max_part_size_bytes = source.max_part_size_bytes;
    model.download_retry_policy = source.download_retry_policy.clone().map(db_json);
    model.pipeline = source.pipeline.clone().map(db_json);
    model.session_complete_pipeline = source.session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = source.paired_segment_pipeline.clone().map(db_json);
    model.offline_check_count = source.offline_check_count;
    model.offline_check_delay_ms = source.offline_check_delay_ms;
    model
}

fn streamer_model(
    existing: Option<&StreamerDbModel>,
    source: &crate::config::backup::StreamerExport,
    platform_id: &str,
    template_id: Option<String>,
) -> StreamerDbModel {
    let mut model = existing
        .cloned()
        .unwrap_or_else(|| StreamerDbModel::new(&source.name, &source.url, platform_id));
    model.name = source.name.clone();
    model.url = source.url.clone();
    model.platform_config_id = platform_id.to_string();
    model.template_config_id = template_id;
    model.state = imported_state(existing, &source.state);
    if clears_error_tracking(existing, &model.state) {
        model.consecutive_error_count = Some(0);
        model.disabled_until = None;
        model.last_error = None;
    }
    model.priority = source.priority.to_ascii_uppercase();
    model.avatar = source
        .avatar_url
        .clone()
        .filter(|value| !value.trim().is_empty());
    model.streamer_specific_config = source.streamer_specific_config.clone().map(db_json);
    model.updated_at = crate::database::time::now_ms();
    model
}

/// Value written to `streamers.state`.
///
/// Only `DISABLED` and `CANCELLED` are carried over from the bundle: every other
/// [`StreamerState`] is produced by monitoring, so importing one would overwrite
/// the state of a streamer whose actor is mid-check or whose download is running —
/// a `LIVE` row rewritten to `NOT_LIVE` re-triggers `SessionLifecycle::on_live_detected`
/// on the next poll. An existing row otherwise keeps its own state, except when the
/// bundle re-enables a row the operator had stopped, which resets it to `NOT_LIVE`
/// so `StreamerMetadata::is_active` turns true again.
fn imported_state(existing: Option<&StreamerDbModel>, source_state: &str) -> String {
    let stopped =
        |state: StreamerState| matches!(state, StreamerState::Disabled | StreamerState::Cancelled);
    let not_live = StreamerState::NotLive.as_str().to_string();

    match StreamerState::parse(source_state) {
        Some(imported) if stopped(imported) => imported.as_str().to_string(),
        _ => match existing.and_then(|row| StreamerState::parse(&row.state)) {
            Some(current) if stopped(current) => not_live,
            Some(current) => current.as_str().to_string(),
            None => not_live,
        },
    }
}

/// Whether the row's error bookkeeping (`consecutive_error_count`,
/// `disabled_until`, `last_error`) must be dropped alongside the state
/// [`imported_state`] chose.
///
/// `streamer_model` starts from `existing.cloned()`, so without this the backoff
/// a stopped streamer accumulated survives the write and a later re-enable lands
/// on `NOT_LIVE` with a `disabled_until` still in the future, which
/// `StreamerMetadata::is_ready_for_check` keeps rejecting. Matches
/// `StreamerManager::partial_update_streamer`, which calls
/// `StreamerMetadata::clear_error_tracking` on a `Disabled` write.
fn clears_error_tracking(existing: Option<&StreamerDbModel>, new_state: &str) -> bool {
    let stopped =
        |state: StreamerState| matches!(state, StreamerState::Disabled | StreamerState::Cancelled);
    let new_state = StreamerState::parse(new_state);
    if new_state.is_some_and(stopped) {
        return true;
    }
    new_state == Some(StreamerState::NotLive)
        && existing
            .and_then(|row| StreamerState::parse(&row.state))
            .is_some_and(stopped)
}

fn db_json(value: serde_json::Value) -> String {
    crate::config::backup::json_value_to_db_string(value)
}

async fn apply_notification_channels(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.notification_channels {
        let existing = snapshot.channels.get(&item.name);
        let channel_type = ChannelType::parse(&item.channel_type).ok_or_else(|| {
            validation_error(format!(
                "Invalid notification channel type '{}'",
                item.channel_type
            ))
        })?;
        let model = if let Some(existing) = existing {
            NotificationChannelDbModel {
                id: existing.id.clone(),
                name: item.name.clone(),
                channel_type: channel_type.as_str().to_string(),
                settings: db_json(item.settings.clone()),
            }
        } else {
            NotificationChannelDbModel::new(
                &item.name,
                channel_type,
                db_json(item.settings.clone()),
            )
        };
        persist_channel(tx, &model).await?;
        sqlx::query("DELETE FROM notification_subscription WHERE channel_id = ?")
            .bind(&model.id)
            .execute(&mut **tx)
            .await?;
        for event in &item.subscriptions {
            sqlx::query(
                "INSERT INTO notification_subscription (channel_id, event_name) VALUES (?, ?)",
            )
            .bind(&model.id)
            .bind(event)
            .execute(&mut **tx)
            .await?;
        }
        if existing.is_some() {
            stats.channels_updated += 1;
        } else {
            stats.channels_created += 1;
        }
    }

    if replace {
        let imported: HashSet<&str> = config
            .notification_channels
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.channels {
            if imported.contains(name.as_str()) {
                continue;
            }
            sqlx::query("DELETE FROM notification_dead_letter WHERE channel_id = ?")
                .bind(&existing.id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("DELETE FROM notification_subscription WHERE channel_id = ?")
                .bind(&existing.id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("DELETE FROM notification_channel WHERE id = ?")
                .bind(&existing.id)
                .execute(&mut **tx)
                .await?;
            stats.channels_deleted += 1;
        }
    }
    Ok(())
}

async fn apply_job_presets(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.job_presets {
        let existing = snapshot.job_presets.get(&item.name);
        let mut model = existing
            .cloned()
            .unwrap_or_else(|| JobPreset::new(&item.name, &item.processor, item.config.clone()));
        model.name = item.name.clone();
        model.description = item.description.clone();
        model.category = item.category.clone();
        model.processor = item.processor.clone();
        model.config = crate::config::backup::unwrap_json_value(item.config.clone()).to_string();
        model.updated_at = Utc::now();
        persist_job_preset(tx, &model).await?;
        if existing.is_some() {
            stats.job_presets_updated += 1;
        } else {
            stats.job_presets_created += 1;
        }
    }
    if replace {
        let imported: HashSet<&str> = config
            .job_presets
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.job_presets {
            if !imported.contains(name.as_str()) {
                sqlx::query("DELETE FROM job_presets WHERE id = ?")
                    .bind(&existing.id)
                    .execute(&mut **tx)
                    .await?;
                stats.job_presets_deleted += 1;
            }
        }
    }
    Ok(())
}

async fn apply_pipeline_presets(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.pipeline_presets {
        let existing = snapshot.pipeline_presets.get(&item.name);
        let dag_value = item.dag_definition.clone().ok_or_else(|| {
            validation_error(format!(
                "Missing dag_definition for pipeline preset '{}'",
                item.name
            ))
        })?;
        let raw_dag = crate::config::backup::unwrap_json_value(dag_value);
        // Parsing rejects invalid DAGs; the parsed value is persisted only for new
        // presets, where PipelinePreset::new serializes it exactly like the API
        // create handler (api::routes::pipeline::presets::create_pipeline_preset).
        let dag: crate::database::models::job::DagPipelineDefinition =
            serde_json::from_value(raw_dag.clone()).map_err(|error| {
                validation_error(format!(
                    "Invalid dag_definition for pipeline preset '{}': {}",
                    item.name, error
                ))
            })?;
        let mut model = existing
            .cloned()
            .unwrap_or_else(|| PipelinePreset::new(&item.name, dag));
        model.name = item.name.clone();
        model.description = item.description.clone();
        if existing.is_some() {
            // Updates store the bundle's own JSON: round-tripping through
            // DagPipelineDefinition would drop fields the struct does not model.
            model.dag_definition = Some(raw_dag.to_string());
        }
        // pipeline_presets.pipeline_type is NOT NULL and PipelinePreset::new writes
        // "dag", so an absent bundle value coerces to the same default while a
        // present value is stored as-is.
        model.pipeline_type = item
            .pipeline_type
            .clone()
            .or_else(|| Some("dag".to_string()));
        model.updated_at = Utc::now();
        persist_pipeline_preset(tx, &model).await?;
        if existing.is_some() {
            stats.pipeline_presets_updated += 1;
        } else {
            stats.pipeline_presets_created += 1;
        }
    }
    if replace {
        let imported: HashSet<&str> = config
            .pipeline_presets
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.pipeline_presets {
            if !imported.contains(name.as_str()) {
                sqlx::query("DELETE FROM pipeline_presets WHERE id = ?")
                    .bind(&existing.id)
                    .execute(&mut **tx)
                    .await?;
                stats.pipeline_presets_deleted += 1;
            }
        }
    }
    Ok(())
}

async fn apply_users(
    tx: &mut ImmediateTransaction,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    let includes_users = crate::config::backup::schema_version_at_least(&config.version, (0, 1, 3))
        && !config.users.is_empty();
    if !includes_users {
        return Ok(());
    }

    for item in &config.users {
        let existing = snapshot.users.get(&item.username);
        let roles = serde_json::to_string(&item.roles).map_err(|error| {
            validation_error(format!(
                "Failed to serialize roles for user '{}': {}",
                item.username, error
            ))
        })?;
        let model = UserDbModel {
            id: existing
                .map(|user| user.id.clone())
                .unwrap_or_else(|| item.id.clone()),
            username: item.username.clone(),
            password_hash: item.password_hash.clone(),
            email: item.email.clone(),
            roles,
            is_active: item.is_active,
            must_change_password: item.must_change_password,
            last_login_at: item.last_login_at,
            created_at: existing
                .map(|user| user.created_at)
                .unwrap_or(item.created_at),
            updated_at: item.updated_at,
        };
        persist_user(tx, &model).await?;
        if existing.is_some() {
            stats.users_updated += 1;
        } else {
            stats.users_created += 1;
        }
    }

    if replace {
        let imported: HashSet<&str> = config
            .users
            .iter()
            .map(|item| item.username.as_str())
            .collect();
        for (username, existing) in &snapshot.users {
            if !imported.contains(username.as_str()) {
                sqlx::query("DELETE FROM users WHERE id = ?")
                    .bind(&existing.id)
                    .execute(&mut **tx)
                    .await?;
                stats.users_deleted += 1;
            }
        }
    }
    Ok(())
}

async fn persist_global(
    tx: &mut ImmediateTransaction,
    config: &GlobalConfigDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE global_config SET
            output_folder = ?, output_filename_template = ?, output_file_format = ?,
            min_segment_size_bytes = ?, max_download_duration_secs = ?, max_part_size_bytes = ?,
            record_danmu = ?, danmu_statistics = ?, max_concurrent_downloads = ?, max_concurrent_uploads = ?,
            streamer_check_delay_ms = ?, proxy_config = ?, offline_check_delay_ms = ?,
            offline_check_count = ?, default_download_engine = ?, max_concurrent_cpu_jobs = ?,
            max_concurrent_io_jobs = ?, job_history_retention_days = ?,
            notification_event_log_retention_days = ?, pipeline = ?, session_complete_pipeline = ?,
            paired_segment_pipeline = ?, log_filter_directive = ?, auto_thumbnail = ?,
            pipeline_cpu_job_timeout_secs = ?, pipeline_io_job_timeout_secs = ?,
            pipeline_execute_timeout_secs = ?, queue_freshness_threshold_ms = ?,
            gpu_health_probe_interval_secs = ?, stream_proxy_allow_private_targets = ?
        WHERE id = ?
        "#,
    )
    .bind(&config.output_folder)
    .bind(&config.output_filename_template)
    .bind(&config.output_file_format)
    .bind(config.min_segment_size_bytes)
    .bind(config.max_download_duration_secs)
    .bind(config.max_part_size_bytes)
    .bind(config.record_danmu)
    .bind(&config.danmu_statistics)
    .bind(config.max_concurrent_downloads)
    .bind(config.max_concurrent_uploads)
    .bind(config.streamer_check_delay_ms)
    .bind(&config.proxy_config)
    .bind(config.offline_check_delay_ms)
    .bind(config.offline_check_count)
    .bind(&config.default_download_engine)
    .bind(config.max_concurrent_cpu_jobs)
    .bind(config.max_concurrent_io_jobs)
    .bind(config.job_history_retention_days)
    .bind(config.notification_event_log_retention_days)
    .bind(&config.pipeline)
    .bind(&config.session_complete_pipeline)
    .bind(&config.paired_segment_pipeline)
    .bind(&config.log_filter_directive)
    .bind(config.auto_thumbnail)
    .bind(config.pipeline_cpu_job_timeout_secs)
    .bind(config.pipeline_io_job_timeout_secs)
    .bind(config.pipeline_execute_timeout_secs)
    .bind(config.queue_freshness_threshold_ms)
    .bind(config.gpu_health_probe_interval_secs)
    .bind(config.stream_proxy_allow_private_targets)
    .bind(&config.id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_engine(
    tx: &mut ImmediateTransaction,
    model: &EngineConfigurationDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO engine_configuration (id, name, engine_type, config)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            engine_type = excluded.engine_type,
            config = excluded.config
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.engine_type)
    .bind(&model.config)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_template(
    tx: &mut ImmediateTransaction,
    model: &TemplateConfigDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO template_config (
            id, name, output_folder, output_filename_template, cookies, output_file_format,
            min_segment_size_bytes, max_download_duration_secs, max_part_size_bytes,
            record_danmu, danmu_statistics, platform_overrides, download_retry_policy,
            download_engine, engines_override, proxy_config,
            stream_selection_config, pipeline, session_complete_pipeline,
            paired_segment_pipeline, offline_check_count, offline_check_delay_ms,
            created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            output_folder = excluded.output_folder,
            output_filename_template = excluded.output_filename_template,
            cookies = excluded.cookies,
            output_file_format = excluded.output_file_format,
            min_segment_size_bytes = excluded.min_segment_size_bytes,
            max_download_duration_secs = excluded.max_download_duration_secs,
            max_part_size_bytes = excluded.max_part_size_bytes,
            record_danmu = excluded.record_danmu,
            danmu_statistics = excluded.danmu_statistics,
            platform_overrides = excluded.platform_overrides,
            download_retry_policy = excluded.download_retry_policy,
            download_engine = excluded.download_engine,
            engines_override = excluded.engines_override,
            proxy_config = excluded.proxy_config,
            stream_selection_config = excluded.stream_selection_config,
            pipeline = excluded.pipeline,
            session_complete_pipeline = excluded.session_complete_pipeline,
            paired_segment_pipeline = excluded.paired_segment_pipeline,
            offline_check_count = excluded.offline_check_count,
            offline_check_delay_ms = excluded.offline_check_delay_ms,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.output_folder)
    .bind(&model.output_filename_template)
    .bind(&model.cookies)
    .bind(&model.output_file_format)
    .bind(model.min_segment_size_bytes)
    .bind(model.max_download_duration_secs)
    .bind(model.max_part_size_bytes)
    .bind(model.record_danmu)
    .bind(&model.danmu_statistics)
    .bind(&model.platform_overrides)
    .bind(&model.download_retry_policy)
    .bind(&model.download_engine)
    .bind(&model.engines_override)
    .bind(&model.proxy_config)
    .bind(&model.stream_selection_config)
    .bind(&model.pipeline)
    .bind(&model.session_complete_pipeline)
    .bind(&model.paired_segment_pipeline)
    .bind(model.offline_check_count)
    .bind(model.offline_check_delay_ms)
    .bind(model.created_at)
    .bind(model.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_platform(
    tx: &mut ImmediateTransaction,
    model: &PlatformConfigDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE platform_config SET
            platform_name = ?, fetch_delay_ms = ?, download_delay_ms = ?, cookies = ?,
            platform_specific_config = ?, proxy_config = ?, record_danmu = ?, danmu_statistics = ?, output_folder = ?,
            output_filename_template = ?, download_engine = ?, stream_selection_config = ?,
            output_file_format = ?, min_segment_size_bytes = ?, max_download_duration_secs = ?,
            max_part_size_bytes = ?, download_retry_policy = ?, pipeline = ?,
            session_complete_pipeline = ?, paired_segment_pipeline = ?, offline_check_count = ?,
            offline_check_delay_ms = ?
        WHERE id = ?
        "#,
    )
    .bind(&model.platform_name)
    .bind(model.fetch_delay_ms)
    .bind(model.download_delay_ms)
    .bind(&model.cookies)
    .bind(&model.platform_specific_config)
    .bind(&model.proxy_config)
    .bind(model.record_danmu)
    .bind(&model.danmu_statistics)
    .bind(&model.output_folder)
    .bind(&model.output_filename_template)
    .bind(&model.download_engine)
    .bind(&model.stream_selection_config)
    .bind(&model.output_file_format)
    .bind(model.min_segment_size_bytes)
    .bind(model.max_download_duration_secs)
    .bind(model.max_part_size_bytes)
    .bind(&model.download_retry_policy)
    .bind(&model.pipeline)
    .bind(&model.session_complete_pipeline)
    .bind(&model.paired_segment_pipeline)
    .bind(model.offline_check_count)
    .bind(model.offline_check_delay_ms)
    .bind(&model.id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_streamer(
    tx: &mut ImmediateTransaction,
    model: &StreamerDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO streamers (
            id, name, url, platform_config_id, template_config_id, state, priority, avatar,
            last_live_time, streamer_specific_config, consecutive_error_count, disabled_until,
            last_error, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            url = excluded.url,
            platform_config_id = excluded.platform_config_id,
            template_config_id = excluded.template_config_id,
            state = excluded.state,
            priority = excluded.priority,
            avatar = excluded.avatar,
            last_live_time = excluded.last_live_time,
            streamer_specific_config = excluded.streamer_specific_config,
            consecutive_error_count = excluded.consecutive_error_count,
            disabled_until = excluded.disabled_until,
            last_error = excluded.last_error,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.url)
    .bind(&model.platform_config_id)
    .bind(&model.template_config_id)
    .bind(&model.state)
    .bind(&model.priority)
    .bind(&model.avatar)
    .bind(model.last_live_time)
    .bind(&model.streamer_specific_config)
    .bind(model.consecutive_error_count)
    .bind(model.disabled_until)
    .bind(&model.last_error)
    .bind(model.created_at)
    .bind(model.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_filter(
    tx: &mut ImmediateTransaction,
    model: &FilterDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO filters (id, streamer_id, filter_type, config) VALUES (?, ?, ?, ?)")
        .bind(&model.id)
        .bind(&model.streamer_id)
        .bind(&model.filter_type)
        .bind(&model.config)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn persist_channel(
    tx: &mut ImmediateTransaction,
    model: &NotificationChannelDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO notification_channel (id, name, channel_type, settings)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            channel_type = excluded.channel_type,
            settings = excluded.settings
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.channel_type)
    .bind(&model.settings)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_job_preset(
    tx: &mut ImmediateTransaction,
    model: &JobPreset,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO job_presets
            (id, name, description, category, processor, config, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            description = excluded.description,
            category = excluded.category,
            processor = excluded.processor,
            config = excluded.config,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.description)
    .bind(&model.category)
    .bind(&model.processor)
    .bind(&model.config)
    .bind(model.created_at)
    .bind(model.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_pipeline_preset(
    tx: &mut ImmediateTransaction,
    model: &PipelinePreset,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO pipeline_presets
            (id, name, description, dag_definition, pipeline_type, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            description = excluded.description,
            dag_definition = excluded.dag_definition,
            pipeline_type = excluded.pipeline_type,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&model.id)
    .bind(&model.name)
    .bind(&model.description)
    .bind(&model.dag_definition)
    .bind(&model.pipeline_type)
    .bind(model.created_at)
    .bind(model.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_user(
    tx: &mut ImmediateTransaction,
    model: &UserDbModel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO users (
            id, username, password_hash, email, roles, is_active,
            must_change_password, last_login_at, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            username = excluded.username,
            password_hash = excluded.password_hash,
            email = excluded.email,
            roles = excluded.roles,
            is_active = excluded.is_active,
            must_change_password = excluded.must_change_password,
            last_login_at = excluded.last_login_at,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&model.id)
    .bind(&model.username)
    .bind(&model.password_hash)
    .bind(&model.email)
    .bind(&model.roles)
    .bind(model.is_active)
    .bind(model.must_change_password)
    .bind(model.last_login_at)
    .bind(model.created_at)
    .bind(model.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::backup::{GlobalConfigExport, JobPresetExport, UserExport};
    use crate::database::{init_pool_with_size, run_migrations};

    const VALID_PASSWORD_HASH: &str = concat!(
        "$argon2id$v=19$m=19456,t=2,p=1$",
        "c2FsdHNhbHRzYWx0c2FsdA$",
        "TiYF50X49Bmdr2XJZ6yBn4ozsFFV8gGZJIXZ3Jg9pWg"
    );

    fn import_config(global: &GlobalConfigDbModel) -> ConfigExport {
        ConfigExport {
            version: "0.1.7".to_string(),
            exported_at: Utc::now().to_rfc3339(),
            global_config: GlobalConfigExport {
                output_folder: global.output_folder.clone(),
                output_filename_template: global.output_filename_template.clone(),
                output_file_format: global.output_file_format.clone(),
                min_segment_size_bytes: global.min_segment_size_bytes,
                max_download_duration_secs: global.max_download_duration_secs,
                max_part_size_bytes: global.max_part_size_bytes,
                record_danmu: global.record_danmu,
                danmu_statistics: None,
                max_concurrent_downloads: global.max_concurrent_downloads,
                max_concurrent_uploads: global.max_concurrent_uploads,
                streamer_check_delay_ms: global.streamer_check_delay_ms,
                proxy_config: serde_json::from_str(&global.proxy_config)
                    .unwrap_or(serde_json::Value::Null),
                offline_check_delay_ms: global.offline_check_delay_ms,
                offline_check_count: global.offline_check_count,
                default_download_engine: global.default_download_engine.clone(),
                max_concurrent_cpu_jobs: global.max_concurrent_cpu_jobs,
                max_concurrent_io_jobs: global.max_concurrent_io_jobs,
                job_history_retention_days: global.job_history_retention_days,
                notification_event_log_retention_days: global.notification_event_log_retention_days,
                pipeline: None,
                session_complete_pipeline: None,
                paired_segment_pipeline: None,
                log_filter_directive: Some(global.log_filter_directive.clone()),
                auto_thumbnail: global.auto_thumbnail,
                pipeline_cpu_job_timeout_secs: global.pipeline_cpu_job_timeout_secs,
                pipeline_io_job_timeout_secs: global.pipeline_io_job_timeout_secs,
                pipeline_execute_timeout_secs: global.pipeline_execute_timeout_secs,
                queue_freshness_threshold_ms: global.queue_freshness_threshold_ms,
                gpu_health_probe_interval_secs: global.gpu_health_probe_interval_secs,
                stream_proxy_allow_private_targets: global.stream_proxy_allow_private_targets,
            },
            templates: Vec::new(),
            streamers: Vec::new(),
            engines: Vec::new(),
            platforms: Vec::new(),
            notification_channels: Vec::new(),
            job_presets: Vec::new(),
            pipeline_presets: Vec::new(),
            users: Vec::new(),
        }
    }

    fn imported_user(username: &str, roles: Vec<String>, is_active: bool) -> UserExport {
        UserExport {
            id: format!("{username}-id"),
            username: username.to_string(),
            password_hash: VALID_PASSWORD_HASH.to_string(),
            email: None,
            roles,
            is_active,
            must_change_password: false,
            last_login_at: None,
            created_at: 1_767_225_600_000,
            updated_at: 1_767_225_600_000,
        }
    }

    fn imported_streamer(url: &str, platform: &str) -> crate::config::backup::StreamerExport {
        crate::config::backup::StreamerExport {
            name: "imported".to_string(),
            url: url.to_string(),
            platform: platform.to_string(),
            template: None,
            priority: "NORMAL".to_string(),
            state: "NOT_LIVE".to_string(),
            avatar_url: None,
            streamer_specific_config: None,
            filters: Vec::new(),
        }
    }

    /// Seeds two streamer rows whose URLs differ only by ASCII case, ids chosen so
    /// "streamer-a" is the lowest-id row of the group. The migrated schema declares
    /// `streamers.url` COLLATE NOCASE UNIQUE, which rejects such rows outright, but
    /// databases whose streamers table predates that collation enforce
    /// case-sensitive uniqueness only — recreate that table shape (CHECK/FK
    /// constraints elided) so the rows can exist.
    async fn seed_case_duplicate_streamers(pool: &SqlitePool) -> PlatformConfigDbModel {
        sqlx::query("DROP TABLE streamers")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE streamers (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                platform_config_id TEXT NOT NULL,
                template_config_id TEXT,
                state TEXT NOT NULL,
                priority TEXT NOT NULL DEFAULT 'NORMAL',
                last_live_time INTEGER,
                streamer_specific_config TEXT,
                consecutive_error_count INTEGER DEFAULT 0,
                last_error TEXT,
                disabled_until INTEGER,
                avatar TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
                updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();

        let platform: PlatformConfigDbModel =
            sqlx::query_as("SELECT * FROM platform_config WHERE platform_name = 'huya'")
                .fetch_one(pool)
                .await
                .unwrap();
        let mut lower =
            StreamerDbModel::new("lower", "https://example.com/live/alpha", &platform.id);
        lower.id = "streamer-a".to_string();
        let mut mixed =
            StreamerDbModel::new("mixed", "https://example.com/live/Alpha", &platform.id);
        mixed.id = "streamer-b".to_string();
        let mut setup_tx = begin_immediate(pool).await.unwrap();
        persist_streamer(&mut setup_tx, &lower).await.unwrap();
        persist_streamer(&mut setup_tx, &mixed).await.unwrap();
        setup_tx.commit().await.unwrap();
        platform
    }

    /// Replace-mode `validate_references` resolves engine names from the bundle
    /// alone, so a replace bundle must mirror the seeded engines to keep
    /// `global_config.default_download_engine` valid.
    async fn mirror_engines(pool: &SqlitePool, config: &mut ConfigExport) {
        let engines: Vec<EngineConfigurationDbModel> =
            sqlx::query_as("SELECT * FROM engine_configuration")
                .fetch_all(pool)
                .await
                .unwrap();
        config.engines = engines
            .iter()
            .map(|engine| crate::config::backup::EngineExport {
                name: engine.name.clone(),
                engine_type: engine.engine_type.clone(),
                config: serde_json::from_str(&engine.config).unwrap(),
            })
            .collect();
    }

    /// Records every `stop_for_removal` call together with whether the streamer's
    /// row was still present when it ran, so tests can assert the import stops the
    /// runtime before its transaction deletes the row.
    struct RecordingRemovalStop {
        pool: SqlitePool,
        calls: std::sync::Mutex<Vec<(String, bool)>>,
        /// Error returned by every `stop_for_removal` call, for the case where
        /// the runtime cannot release a streamer the bundle removes.
        failure: std::sync::Mutex<Option<String>>,
        /// Streamer row inserted from inside the first `stop_for_removal` call,
        /// standing in for one an operator creates while the import is stopping
        /// recordings.
        insert_during_stop: std::sync::Mutex<Option<StreamerDbModel>>,
    }

    impl RecordingRemovalStop {
        fn calls(&self) -> Vec<(String, bool)> {
            self.calls.lock().unwrap().clone()
        }

        fn fail_with(&self, message: &str) {
            *self.failure.lock().unwrap() = Some(message.to_string());
        }

        fn insert_during_stop(&self, model: StreamerDbModel) {
            *self.insert_during_stop.lock().unwrap() = Some(model);
        }
    }

    #[async_trait::async_trait]
    impl StreamerRemovalStop for RecordingRemovalStop {
        async fn stop_for_removal(&self, streamer_id: &str) -> crate::Result<()> {
            let present: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM streamers WHERE id = ?")
                .bind(streamer_id)
                .fetch_one(&self.pool)
                .await
                .unwrap();
            self.calls
                .lock()
                .unwrap()
                .push((streamer_id.to_string(), present.0 > 0));

            let latecomer = self.insert_during_stop.lock().unwrap().take();
            if let Some(model) = latecomer {
                let mut tx = begin_immediate(&self.pool).await.unwrap();
                persist_streamer(&mut tx, &model).await.unwrap();
                tx.commit().await.unwrap();
            }

            let failure = self.failure.lock().unwrap().clone();
            match failure {
                Some(message) => Err(crate::Error::Other(message)),
                None => Ok(()),
            }
        }
    }

    /// A migrated in-memory database wired to the same collaborators
    /// `ServiceContainer::build_api_state` gives the import service, plus the
    /// event stream and removal-stop log the tests assert on.
    struct ImportHarness {
        pool: SqlitePool,
        global: GlobalConfigDbModel,
        service: ConfigurationImportService,
        streamer_manager: Arc<RuntimeStreamerManager>,
        removal_stop: Arc<RecordingRemovalStop>,
        events: tokio::sync::broadcast::Receiver<crate::config::ConfigUpdateEvent>,
    }

    impl ImportHarness {
        async fn new() -> Self {
            let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
            run_migrations(&pool).await.unwrap();
            let global: GlobalConfigDbModel =
                sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();

            let broadcaster = crate::config::ConfigEventBroadcaster::new();
            let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
            let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
            let config_service = Arc::new(RuntimeConfigService::with_cache_and_broadcaster(
                config_repo.clone(),
                streamer_repo.clone(),
                crate::config::ConfigCache::new(),
                broadcaster.clone(),
            ));
            let streamer_manager = Arc::new(RuntimeStreamerManager::new(
                streamer_repo,
                broadcaster.clone(),
            ));
            let credential_service = Arc::new(RuntimeCredentialService::new(
                Arc::new(crate::credentials::CredentialResolver::new(config_repo)),
                Arc::new(crate::database::repositories::SqlxCredentialStore::new(
                    pool.clone(),
                    pool.clone(),
                )),
            ));
            let removal_stop = Arc::new(RecordingRemovalStop {
                pool: pool.clone(),
                calls: std::sync::Mutex::new(Vec::new()),
                failure: std::sync::Mutex::new(None),
                insert_during_stop: std::sync::Mutex::new(None),
            });

            let service = ConfigurationImportService::new(
                pool.clone(),
                config_service,
                streamer_manager.clone(),
                Arc::new(NotificationService::new()),
                credential_service,
                removal_stop.clone(),
            );

            Self {
                pool,
                global,
                service,
                streamer_manager,
                removal_stop,
                events: broadcaster.subscribe(),
            }
        }

        /// Inserts a streamer row on the `huya` platform and registers it with
        /// `streamer_manager`, standing in for a streamer the runtime is already
        /// monitoring when the import arrives.
        async fn seed_streamer(&self, url: &str, state: StreamerState) -> StreamerDbModel {
            let platform: PlatformConfigDbModel =
                sqlx::query_as("SELECT * FROM platform_config WHERE platform_name = 'huya'")
                    .fetch_one(&self.pool)
                    .await
                    .unwrap();
            let mut model = StreamerDbModel::new("existing", url, &platform.id);
            model.state = state.as_str().to_string();
            let mut tx = begin_immediate(&self.pool).await.unwrap();
            persist_streamer(&mut tx, &model).await.unwrap();
            tx.commit().await.unwrap();
            self.streamer_manager
                .reload_from_repo(&model.id)
                .await
                .unwrap();
            model
        }

        fn drain_events(&mut self) -> Vec<crate::config::ConfigUpdateEvent> {
            let mut events = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                events.push(event);
            }
            events
        }
    }

    /// io::Write sink shared with a tracing fmt subscriber so tests can assert on
    /// emitted log lines.
    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for SharedLogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn validation_rejects_duplicate_streamer_urls_case_insensitively() {
        let global = GlobalConfigDbModel::default();
        let mut config = import_config(&global);
        config.streamers = vec![
            crate::config::backup::StreamerExport {
                name: "one".to_string(),
                url: "https://example.com/Live".to_string(),
                platform: "test".to_string(),
                template: None,
                priority: "NORMAL".to_string(),
                state: "NOT_LIVE".to_string(),
                avatar_url: None,
                streamer_specific_config: None,
                filters: Vec::new(),
            },
            crate::config::backup::StreamerExport {
                name: "two".to_string(),
                url: "https://example.com/live".to_string(),
                platform: "test".to_string(),
                template: None,
                priority: "NORMAL".to_string(),
                state: "NOT_LIVE".to_string(),
                avatar_url: None,
                streamer_specific_config: None,
                filters: Vec::new(),
            },
        ];

        assert!(matches!(
            validate_import(&config, ImportMode::Merge),
            Err(ConfigurationImportError::Validation(message))
                if message.contains("Duplicate streamer URL")
        ));
    }

    #[test]
    fn validation_rejects_non_argon2id_password_hashes() {
        let global = GlobalConfigDbModel::default();
        let mut config = import_config(&global);
        let mut user = imported_user("admin", vec!["admin".to_string()], true);
        user.password_hash = "plaintext-password".to_string();
        config.users.push(user);

        assert!(matches!(
            validate_import(&config, ImportMode::Merge),
            Err(ConfigurationImportError::Validation(message))
                if message.contains("Invalid password hash")
        ));
    }

    #[test]
    fn validation_rejects_usernames_longer_than_login_accepts() {
        let global = GlobalConfigDbModel::default();
        let mut config = import_config(&global);
        config.users.push(imported_user(
            &"a".repeat(MAX_USERNAME_LENGTH + 1),
            vec!["admin".to_string()],
            true,
        ));

        assert!(matches!(
            validate_import(&config, ImportMode::Merge),
            Err(ConfigurationImportError::Validation(message))
                if message.contains("exceeds")
        ));
    }

    #[test]
    fn validation_measures_usernames_in_characters() {
        let global = GlobalConfigDbModel::default();
        let mut config = import_config(&global);
        // 128 CJK characters are 384 bytes; the bound is on characters, so
        // this name is accepted here and by `AuthService::authenticate`.
        config.users.push(imported_user(
            &"用".repeat(MAX_USERNAME_LENGTH),
            vec!["admin".to_string()],
            true,
        ));

        assert!(validate_import(&config, ImportMode::Merge).is_ok());
    }

    #[test]
    fn replace_validation_requires_an_active_admin() {
        let global = GlobalConfigDbModel::default();
        let mut config = import_config(&global);
        config
            .users
            .push(imported_user("viewer", vec!["user".to_string()], true));

        assert!(matches!(
            validate_import(&config, ImportMode::Replace),
            Err(ConfigurationImportError::Validation(message))
                if message.contains("at least one active user")
        ));
    }

    #[tokio::test]
    async fn successful_import_revokes_refresh_tokens_without_replacing_omitted_users() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let global: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let user = UserDbModel::new(
            "preserved-user",
            VALID_PASSWORD_HASH,
            vec!["user".to_string()],
        );
        let mut setup_tx = begin_immediate(&pool).await.unwrap();
        persist_user(&mut setup_tx, &user).await.unwrap();
        setup_tx.commit().await.unwrap();
        sqlx::query(
            r#"
            INSERT INTO refresh_tokens
                (id, user_id, token_hash, expires_at, created_at, revoked_at, device_info)
            VALUES ('token-id', ?, 'token-hash', ?, ?, NULL, NULL)
            "#,
        )
        .bind(&user.id)
        .bind(crate::database::time::now_ms() + 60_000)
        .bind(crate::database::time::now_ms())
        .execute(&pool)
        .await
        .unwrap();

        let config = import_config(&global);
        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&config, ImportMode::Merge)
            .unwrap();
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE id = ?")
            .bind(&user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let token_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM refresh_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(user_count.0, 1);
        assert_eq!(token_count.0, 0);
    }

    #[tokio::test]
    async fn late_database_failure_rolls_back_earlier_import_writes() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let before: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            r#"
            CREATE TRIGGER reject_imported_job_preset
            BEFORE INSERT ON job_presets
            WHEN NEW.name = 'rollback-trigger'
            BEGIN
                SELECT RAISE(ABORT, 'forced import failure');
            END
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut config = import_config(&before);
        config.global_config.output_folder = "/should-roll-back".to_string();
        config.job_presets.push(JobPresetExport {
            name: "rollback-trigger".to_string(),
            description: None,
            category: None,
            processor: "remux".to_string(),
            config: serde_json::json!({}),
        });
        validate_import(&config, ImportMode::Merge).unwrap();

        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&config, ImportMode::Merge)
            .unwrap();
        let error = apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .expect_err("trigger should abort the import");
        assert!(matches!(error, ConfigurationImportError::Database(_)));
        tx.rollback().await.unwrap();

        let after: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after.output_folder, before.output_folder);
        let inserted: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM job_presets WHERE name = 'rollback-trigger'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(inserted.0, 0);
    }

    #[tokio::test]
    async fn replace_import_registers_a_new_streamer_with_the_runtime() {
        let mut harness = ImportHarness::new().await;
        let mut config = import_config(&harness.global);
        mirror_engines(&harness.pool, &mut config).await;
        config
            .streamers
            .push(imported_streamer("https://example.com/live/new", "huya"));

        let outcome = harness
            .service
            .import(config, ImportMode::Replace)
            .await
            .unwrap();

        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        assert_eq!(outcome.stats.streamers_created, 1);
        let metadata = harness
            .streamer_manager
            .get_streamer_by_url("https://example.com/live/new")
            .expect("imported streamer should be registered without a restart");
        assert_eq!(metadata.state, StreamerState::NotLive);
        assert!(harness.drain_events().contains(
            &crate::config::ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: metadata.id.clone(),
            }
        ));
    }

    #[tokio::test]
    async fn merge_import_without_streamers_leaves_a_live_streamer_live() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        harness
            .service
            .import(import_config(&harness.global), ImportMode::Merge)
            .await
            .unwrap();

        let row: StreamerDbModel = sqlx::query_as("SELECT * FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(row.state, StreamerState::Live.as_str());
        assert_eq!(
            harness
                .streamer_manager
                .get_streamer(&seeded.id)
                .map(|metadata| metadata.state),
            Some(StreamerState::Live)
        );
        assert!(harness.removal_stop.calls().is_empty());
    }

    #[tokio::test]
    async fn import_keeps_the_stored_state_of_a_streamer_the_bundle_updates() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        let mut config = import_config(&harness.global);
        // The bundle carries NOT_LIVE, the state every export of an offline
        // streamer holds; the running recording's LIVE row must survive it.
        config
            .streamers
            .push(imported_streamer(&seeded.url, "huya"));
        let outcome = harness
            .service
            .import(config, ImportMode::Merge)
            .await
            .unwrap();

        assert_eq!(outcome.stats.streamers_updated, 1);
        let row: StreamerDbModel = sqlx::query_as("SELECT * FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(row.state, StreamerState::Live.as_str());
        assert_eq!(row.name, "imported");
    }

    #[tokio::test]
    async fn import_applies_a_bundle_that_disables_an_existing_streamer() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::NotLive)
            .await;

        let mut config = import_config(&harness.global);
        let mut disabled = imported_streamer(&seeded.url, "huya");
        disabled.state = StreamerState::Disabled.as_str().to_string();
        config.streamers.push(disabled);
        harness
            .service
            .import(config, ImportMode::Merge)
            .await
            .unwrap();

        let row: StreamerDbModel = sqlx::query_as("SELECT * FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(row.state, StreamerState::Disabled.as_str());
        // ConfigEventHandler decides on cleanup from the cached metadata's
        // `is_active`, so the disable has to reach it too.
        assert_eq!(
            harness
                .streamer_manager
                .get_streamer(&seeded.id)
                .map(|metadata| metadata.is_active()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn replace_import_stops_and_evicts_a_streamer_the_bundle_omits() {
        let mut harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        let mut config = import_config(&harness.global);
        mirror_engines(&harness.pool, &mut config).await;
        let outcome = harness
            .service
            .import(config, ImportMode::Replace)
            .await
            .unwrap();

        assert_eq!(outcome.stats.streamers_deleted, 1);
        assert_eq!(
            harness.removal_stop.calls(),
            vec![(seeded.id.clone(), true)],
            "the recording must be stopped while the row still exists"
        );
        assert!(harness.streamer_manager.get_streamer(&seeded.id).is_none());
        assert!(
            harness
                .streamer_manager
                .get_streamer_by_url(&seeded.url)
                .is_none()
        );
        assert!(harness.drain_events().contains(
            &crate::config::ConfigUpdateEvent::StreamerDeleted {
                streamer_id: seeded.id.clone(),
            }
        ));
    }

    #[tokio::test]
    async fn rejected_replace_import_leaves_the_runtime_untouched() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        // No `engines` section, as an export from an older schema carries, so
        // replace-mode `validate_references` cannot resolve
        // `global_config.default_download_engine`.
        let config = import_config(&harness.global);
        let error = harness
            .service
            .import(config, ImportMode::Replace)
            .await
            .expect_err("replace import without engines should be rejected");

        assert!(matches!(error, ConfigurationImportError::Validation(_)));
        assert!(
            harness.removal_stop.calls().is_empty(),
            "a rejected import must not stop any recording"
        );
        let row: StreamerDbModel = sqlx::query_as("SELECT * FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(row.state, StreamerState::Live.as_str());
    }

    #[tokio::test]
    async fn replace_import_aborts_when_a_removed_streamer_cannot_be_stopped() {
        let harness = ImportHarness::new().await;
        harness.removal_stop.fail_with("engine still running");
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        let mut config = import_config(&harness.global);
        mirror_engines(&harness.pool, &mut config).await;
        let error = harness
            .service
            .import(config, ImportMode::Replace)
            .await
            .expect_err("a failed stop should abort the import");

        assert!(matches!(error, ConfigurationImportError::StreamerStop(_)));
        assert_eq!(
            harness.removal_stop.calls(),
            vec![(seeded.id.clone(), true)]
        );
        let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(remaining.0, 1, "the row must survive an aborted import");
    }

    #[tokio::test]
    async fn replace_import_aborts_when_a_streamer_appears_while_stopping() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::Live)
            .await;

        // Inserted from inside the stop hook, so the preflight snapshot never saw
        // it and nothing stopped its runtime; the transaction's delete set must
        // not silently grow to include it.
        let platform: PlatformConfigDbModel =
            sqlx::query_as("SELECT * FROM platform_config WHERE platform_name = 'huya'")
                .fetch_one(&harness.pool)
                .await
                .unwrap();
        let mut latecomer =
            StreamerDbModel::new("latecomer", "https://example.com/live/beta", &platform.id);
        latecomer.id = "streamer-latecomer".to_string();
        harness.removal_stop.insert_during_stop(latecomer.clone());

        let mut config = import_config(&harness.global);
        mirror_engines(&harness.pool, &mut config).await;
        let error = harness
            .service
            .import(config, ImportMode::Replace)
            .await
            .expect_err("an unstopped streamer in the delete set should abort the import");

        assert!(matches!(error, ConfigurationImportError::StreamerStop(_)));
        assert_eq!(
            harness.removal_stop.calls(),
            vec![(seeded.id.clone(), true)]
        );
        let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
            .fetch_all(&harness.pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "the transaction must roll back");
        assert!(
            rows.iter()
                .any(|row| row.id == seeded.id && row.state == StreamerState::Live.as_str())
        );
        assert!(rows.iter().any(|row| row.id == latecomer.id));
    }

    #[tokio::test]
    async fn import_clears_the_error_backoff_of_a_streamer_it_stops() {
        let harness = ImportHarness::new().await;
        let seeded = harness
            .seed_streamer("https://example.com/live/alpha", StreamerState::NotLive)
            .await;
        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 4, disabled_until = ?, \
             last_error = 'boom' WHERE id = ?",
        )
        .bind(crate::database::time::now_ms() + 3_600_000)
        .bind(&seeded.id)
        .execute(&harness.pool)
        .await
        .unwrap();

        let mut config = import_config(&harness.global);
        let mut disabled = imported_streamer(&seeded.url, "huya");
        disabled.state = StreamerState::Disabled.as_str().to_string();
        config.streamers.push(disabled);
        harness
            .service
            .import(config, ImportMode::Merge)
            .await
            .unwrap();

        let row: StreamerDbModel = sqlx::query_as("SELECT * FROM streamers WHERE id = ?")
            .bind(&seeded.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        assert_eq!(row.state, StreamerState::Disabled.as_str());
        assert_eq!(row.consecutive_error_count, Some(0));
        assert_eq!(row.disabled_until, None);
        assert_eq!(row.last_error, None);
    }

    #[test]
    fn clears_error_tracking_follows_the_stop_and_re_enable_writes() {
        let mut not_live =
            StreamerDbModel::new("not live", "https://example.com/live", "platform-huya");
        not_live.state = StreamerState::NotLive.as_str().to_string();
        let mut disabled =
            StreamerDbModel::new("disabled", "https://example.com/other", "platform-huya");
        disabled.state = StreamerState::Disabled.as_str().to_string();

        assert!(clears_error_tracking(
            Some(&not_live),
            StreamerState::Disabled.as_str()
        ));
        assert!(clears_error_tracking(
            Some(&disabled),
            StreamerState::NotLive.as_str()
        ));
        assert!(!clears_error_tracking(
            Some(&not_live),
            StreamerState::NotLive.as_str()
        ));
        assert!(!clears_error_tracking(
            Some(&not_live),
            StreamerState::Live.as_str()
        ));

        // An error row keeps its backoff: `imported_state` leaves it in ERROR, and
        // only `StreamerManager::clear_error_state` retires that bookkeeping.
        let mut errored =
            StreamerDbModel::new("errored", "https://example.com/third", "platform-huya");
        errored.state = StreamerState::Error.as_str().to_string();
        assert_eq!(
            imported_state(Some(&errored), StreamerState::NotLive.as_str()),
            StreamerState::Error.as_str()
        );
        assert!(!clears_error_tracking(
            Some(&errored),
            StreamerState::Error.as_str()
        ));
    }

    #[test]
    fn imported_state_carries_disable_intent_but_not_runtime_state() {
        let mut live = StreamerDbModel::new("live", "https://example.com/live", "platform-huya");
        live.state = StreamerState::Live.as_str().to_string();
        assert_eq!(
            imported_state(Some(&live), StreamerState::NotLive.as_str()),
            StreamerState::Live.as_str()
        );
        assert_eq!(
            imported_state(Some(&live), StreamerState::Disabled.as_str()),
            StreamerState::Disabled.as_str()
        );

        let mut disabled =
            StreamerDbModel::new("disabled", "https://example.com/other", "platform-huya");
        disabled.state = StreamerState::Disabled.as_str().to_string();
        assert_eq!(
            imported_state(Some(&disabled), StreamerState::NotLive.as_str()),
            StreamerState::NotLive.as_str()
        );

        // A row the bundle creates never starts out LIVE: the actor's
        // NotLive -> Live transition is what starts a download.
        assert_eq!(
            imported_state(None, StreamerState::Live.as_str()),
            StreamerState::NotLive.as_str()
        );
        assert_eq!(
            imported_state(None, StreamerState::Disabled.as_str()),
            StreamerState::Disabled.as_str()
        );
    }

    #[tokio::test]
    async fn replace_import_updates_lowest_id_case_duplicate_and_deletes_shadow() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let global: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let platform = seed_case_duplicate_streamers(&pool).await;

        let mut config = import_config(&global);
        mirror_engines(&pool, &mut config).await;
        config.streamers.push(imported_streamer(
            "https://example.com/live/ALPHA",
            &platform.platform_name,
        ));
        validate_import(&config, ImportMode::Replace).unwrap();

        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&config, ImportMode::Replace)
            .unwrap();
        let (stats, _, _) = apply_import(&mut tx, &snapshot, &config, ImportMode::Replace)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "streamer-a");
        assert_eq!(rows[0].url, "https://example.com/live/ALPHA");
        assert_eq!(stats.streamers_updated, 1);
        assert_eq!(stats.streamers_created, 0);
        assert_eq!(stats.streamers_deleted, 1);
    }

    #[tokio::test]
    async fn merge_import_updates_one_case_duplicate_keeps_shadow_and_warns() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let global: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let platform = seed_case_duplicate_streamers(&pool).await;

        let mut config = import_config(&global);
        config.streamers.push(imported_streamer(
            "https://example.com/live/ALPHA",
            &platform.platform_name,
        ));
        validate_import(&config, ImportMode::Merge).unwrap();

        let log_buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer({
                let log_buffer = log_buffer.clone();
                move || log_buffer.clone()
            })
            .finish();
        let _log_guard = tracing::subscriber::set_default(subscriber);

        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&config, ImportMode::Merge)
            .unwrap();
        let (stats, _, _) = apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "streamer-a");
        assert_eq!(rows[0].url, "https://example.com/live/ALPHA");
        assert_eq!(rows[1].id, "streamer-b");
        assert_eq!(rows[1].url, "https://example.com/live/Alpha");
        assert_eq!(stats.streamers_updated, 1);
        assert_eq!(stats.streamers_created, 0);
        assert_eq!(stats.streamers_deleted, 0);

        let logs = log_buffer.contents();
        assert!(
            logs.contains("https://example.com/live/alpha")
                && logs.contains("https://example.com/live/Alpha"),
            "warning should name the colliding URLs, got: {logs}"
        );
    }

    #[tokio::test]
    async fn pipeline_preset_update_round_trips_bundle_dag_definition() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let global: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();

        let mut create_config = import_config(&global);
        create_config
            .pipeline_presets
            .push(crate::config::backup::PipelinePresetExport {
                name: "roundtrip-preset".to_string(),
                description: None,
                dag_definition: Some(serde_json::json!({
                    "name": "roundtrip",
                    "steps": [
                        {"id": "remux", "step": {"type": "preset", "name": "hq_remux"}}
                    ]
                })),
                pipeline_type: None,
            });
        validate_import(&create_config, ImportMode::Merge).unwrap();
        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&create_config, ImportMode::Merge)
            .unwrap();
        apply_import(&mut tx, &snapshot, &create_config, ImportMode::Merge)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // "x_layout" is not a DagPipelineDefinition field; storing the bundle's raw
        // JSON on update must keep it verbatim.
        let updated_dag = serde_json::json!({
            "name": "roundtrip",
            "steps": [
                {
                    "id": "remux",
                    "step": {"type": "preset", "name": "hq_remux"},
                    "depends_on": []
                }
            ],
            "x_layout": {"remux": [10, 20]}
        });
        let mut update_config = import_config(&global);
        update_config
            .pipeline_presets
            .push(crate::config::backup::PipelinePresetExport {
                name: "roundtrip-preset".to_string(),
                description: Some("updated".to_string()),
                dag_definition: Some(updated_dag.clone()),
                pipeline_type: Some("legacy".to_string()),
            });
        validate_import(&update_config, ImportMode::Merge).unwrap();
        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot
            .validate_references(&update_config, ImportMode::Merge)
            .unwrap();
        let (stats, _, _) = apply_import(&mut tx, &snapshot, &update_config, ImportMode::Merge)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(stats.pipeline_presets_updated, 1);
        let stored: PipelinePreset =
            sqlx::query_as("SELECT * FROM pipeline_presets WHERE name = 'roundtrip-preset'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            stored.dag_definition.as_deref(),
            Some(updated_dag.to_string().as_str())
        );
        assert_eq!(stored.pipeline_type.as_deref(), Some("legacy"));
    }
}
