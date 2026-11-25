//! Database snapshot loading and bundle/reference validation for configuration import.

use std::collections::{HashMap, HashSet};

use super::{ConfigurationImportError, profiles, proxies, validation, validation_error};
use crate::config::backup::{
    ConfigExport, ImportMode, NotificationChannelExport, PipelinePresetExport,
};
use crate::database::models::{
    ChannelType, EngineConfigurationDbModel, EngineType, FilterDbModel, FilterType,
    GlobalConfigDbModel, JobPreset, NotificationChannelDbModel, PipelinePreset,
    PlatformConfigDbModel, RetentionDays, StreamerDbModel, TemplateConfigDbModel,
};
use crate::domain::MAX_USERNAME_LENGTH;

pub(super) struct ImportSnapshot {
    pub(super) global: GlobalConfigDbModel,
    pub(super) engines: HashMap<String, EngineConfigurationDbModel>,
    pub(super) templates: HashMap<String, TemplateConfigDbModel>,
    pub(super) platforms: HashMap<String, PlatformConfigDbModel>,
    // Grouped by url.to_ascii_lowercase() and sorted by id.
    pub(super) streamers: HashMap<String, Vec<StreamerDbModel>>,
    // Lowercased urls of rows carrying a `deleted_at` marker.
    pub(super) retiring_streamer_urls: HashSet<String>,
    // Ids of streamers with a stored account selection.
    pub(super) selected_streamers: HashSet<String>,
    // Saved proxies, which a converted older backup reuses by exit.
    pub(super) proxies: Vec<(String, crate::proxies::ProxyEndpoint)>,
    pub(super) channels: HashMap<String, NotificationChannelDbModel>,
    pub(super) job_presets: HashMap<String, JobPreset>,
    pub(super) pipeline_presets: HashMap<String, PipelinePreset>,
}

impl ImportSnapshot {
    pub(super) async fn load(tx: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let global = sqlx::query_as::<_, GlobalConfigDbModel>(
            "SELECT * FROM global_config ORDER BY rowid LIMIT 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        let engines =
            sqlx::query_as::<_, EngineConfigurationDbModel>("SELECT * FROM engine_configuration")
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();
        let templates = sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), model))
            .collect();
        let platforms = sqlx::query_as::<_, PlatformConfigDbModel>("SELECT * FROM platform_config")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|model| (model.platform_name.clone(), model))
            .collect();
        let mut streamers: HashMap<String, Vec<StreamerDbModel>> = HashMap::new();
        for model in
            sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE deleted_at IS NULL")
                .fetch_all(&mut *tx)
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
        let retiring_streamer_urls = sqlx::query_scalar::<_, String>(
            "SELECT url FROM streamers WHERE deleted_at IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|url| url.to_ascii_lowercase())
        .collect();
        let selected_streamers = sqlx::query_scalar::<_, String>(
            "SELECT streamer_id FROM credential_selections WHERE streamer_id IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let proxies = proxies::existing(tx).await.map_err(|error| match error {
            ConfigurationImportError::Database(error) => error,
            error @ (ConfigurationImportError::Validation(_)
            | ConfigurationImportError::MissingProxy(_)) => {
                sqlx::Error::Protocol(error.to_string())
            }
        })?;
        let channels =
            sqlx::query_as::<_, NotificationChannelDbModel>("SELECT * FROM notification_channel")
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();
        let job_presets = sqlx::query_as::<_, JobPreset>("SELECT * FROM job_presets")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), model))
            .collect();
        let pipeline_presets =
            sqlx::query_as::<_, PipelinePreset>("SELECT * FROM pipeline_presets")
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect();

        Ok(Self {
            global,
            engines,
            templates,
            platforms,
            streamers,
            retiring_streamer_urls,
            selected_streamers,
            proxies,
            channels,
            job_presets,
            pipeline_presets,
        })
    }

    pub(super) fn streamer_for_update(&self, url: &str) -> Option<&StreamerDbModel> {
        let rows = self.streamers.get(&url.to_ascii_lowercase())?;
        rows.iter()
            .find(|row| row.url == url)
            .or_else(|| rows.first())
    }

    pub(super) fn upgrade_bundle(
        &self,
        config: &mut ConfigExport,
        mode: ImportMode,
    ) -> Result<(), ConfigurationImportError> {
        crate::services::legacy_upgrade::credentials::upgrade_bundle(config, |streamer| {
            mode == ImportMode::Merge
                && self.streamer_for_update(&streamer.url).is_some_and(|row| {
                    self.selected_streamers.contains(&row.id)
                        && self
                            .platforms
                            .get(&streamer.platform)
                            .is_some_and(|platform| platform.id == row.platform_config_id)
                })
        })?;
        let legacy = config.version.starts_with("0.");
        if legacy || crate::services::legacy_upgrade::proxies::bundle_has_legacy_settings(config) {
            crate::services::legacy_upgrade::proxies::upgrade_bundle(
                config,
                &self.proxies,
                legacy,
            )?;
        }
        config.validate_credential_graph()?;
        Ok(())
    }

    pub(super) fn validate_references(
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
            if self.streamer_for_update(&streamer.url).is_none()
                && self
                    .retiring_streamer_urls
                    .contains(&streamer.url.to_ascii_lowercase())
            {
                return validation(format!(
                    "A streamer with the URL '{}' is still being removed; try again shortly",
                    streamer.url
                ));
            }
        }

        Ok(())
    }
}

pub(super) fn includes_imported_users(config: &ConfigExport) -> bool {
    crate::config::backup::schema_version_at_least(&config.version, (0, 1, 3))
        && !config.users.is_empty()
}

pub(super) fn validate_import(
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    if !config.version.starts_with("0.")
        && config.version != crate::config::backup::MANAGED_CREDENTIAL_SCHEMA_VERSION
    {
        return validation(format!(
            "Unsupported schema version: {}. Expected 0.x or 1.0.0",
            config.version
        ));
    }
    profiles::validate(config)?;
    proxies::validate(config)?;
    RetentionDays::try_from(config.global_config.job_history_retention_days)
        .map_err(|error| validation_error(error.to_string()))?;
    RetentionDays::try_from(config.global_config.notification_event_log_retention_days)
        .map_err(|error| validation_error(error.to_string()))?;

    RetentionDays::try_from(config.global_config.output_retention_days)
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
    if config.global_config.streamer_check_delay_ms < 1 {
        return validation("streamer_check_delay_ms must be positive");
    }
    if config.global_config.offline_check_delay_ms < 1 {
        return validation("offline_check_delay_ms must be positive");
    }
    if let Some(directive) = config.global_config.log_filter_directive.as_deref() {
        validate_non_empty("global log_filter_directive", directive)?;
        crate::logging::validate_filter_directive(directive).map_err(|error| {
            validation_error(format!(
                "Invalid log_filter_directive '{directive}': {error}"
            ))
        })?;
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
        if template
            .offline_check_delay_ms
            .is_some_and(|delay| delay < 1_000)
        {
            return validation(format!(
                "offline_check_delay_ms for template '{}' must be at least 1000",
                template.name
            ));
        }
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
        if platform
            .offline_check_delay_ms
            .is_some_and(|delay| delay < 1_000)
        {
            return validation(format!(
                "offline_check_delay_ms for platform '{}' must be at least 1000",
                platform.platform_name
            ));
        }
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
        crate::domain::StreamerUrl::new(&streamer.url).map_err(|error| {
            validation_error(format!(
                "Invalid URL '{}' for streamer '{}': {error}",
                streamer.url, streamer.name
            ))
        })?;
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
                crate::config::backup::json_value_to_db_string(
                    crate::config::backup::import_filter_config(
                        &config.version,
                        &filter.filter_type,
                        filter.config.clone(),
                    ),
                ),
            );
            let parsed = crate::domain::filter::Filter::try_from(&model).map_err(|error| {
                validation_error(format!(
                    "Invalid {} filter for streamer '{}': {}",
                    filter.filter_type, streamer.name, error
                ))
            })?;
            let timezone = match &parsed {
                crate::domain::filter::Filter::TimeBased(filter) => {
                    Some(filter.timezone.as_deref())
                }
                crate::domain::filter::Filter::Cron(filter) => Some(filter.timezone.as_deref()),
                _ => None,
            };
            if let Some(timezone) = timezone {
                crate::domain::filter::FilterTimezone::parse(timezone).map_err(|error| {
                    validation_error(format!(
                        "Invalid timezone for streamer '{}': {error}",
                        streamer.name
                    ))
                })?;
            }
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

    if includes_imported_users(config) {
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
            false,
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

pub(super) fn validate_unique<'a>(
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
    if !channel_type.supports_proxy()
        && channel
            .proxy_route
            .as_ref()
            .is_some_and(|route| *route != crate::config::backup::BackupRoute::Inherit)
    {
        return validation(format!(
            "Notification channel '{}' is an email channel, which always connects directly; its proxy_route must be inherit",
            channel.name
        ));
    }
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
