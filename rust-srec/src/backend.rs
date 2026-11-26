//! Runtime bootstrap API for backend applications.
//!
//! This module is the supported integration surface for the server binary and
//! desktop shell. Application wiring remains internal so changes to routes,
//! services, and infrastructure do not expand the crate's public API.

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use sqlx::SqlitePool;

use crate::Result;
pub use crate::api::auth_service::{AuthResponse, LocalDesktopAccount};
use crate::api::auth_store::{
    AuthImportError, apply_auth_import_tx, validate_auth_import_tx, validate_user_exports,
};
use crate::api::backup::{ConfigBackupError, ConfigBackupImportOutcome, ConfigBackupPort};
use crate::api::bootstrap::{ApiServerHost, ApiServerRuntime};
use crate::api::playback_context::PlaybackContextService;
pub use crate::api::server::ApiServerConfig;
use crate::api::server::ApiServices;
use crate::config::backup::{ConfigExport, ImportMode, ImportStats, UserExport};
pub use crate::database::{init_database_pools, init_migration_pool, init_pool, init_write_pool};
pub use crate::domain::engine_config::{EngineType, FfmpegEngineConfig, StreamlinkEngineConfig};
pub use crate::downloader::{DownloadManagerConfig, DownloadManagerEvent, DownloadProgressEvent};
pub use crate::logging::init_logging;
pub use crate::notification::{NotificationEvent, NotificationPriority};
pub use crate::panic_hook::install as install_panic_hook;
pub use crate::pipeline::PipelineManagerConfig;
pub use crate::runtime::RuntimeLease;
pub use crate::server::run as run_server;
pub use crate::services::ActiveRecordingSummary;
use crate::services::config_export::ConfigurationExportService;
use crate::services::config_import::{
    AuthImportTxPort, ConfigurationImportError, ConfigurationImportService,
};
use crate::services::container::{DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY};
pub use crate::services::legacy_upgrade::run_migrations;
pub use crate::utils::http_client::install_rustls_provider;

#[cfg(test)]
mod bootstrap_tests;
#[cfg(test)]
mod import_contract_tests;
#[cfg(test)]
mod response_contract_tests;

fn map_auth_import_error(error: AuthImportError) -> ConfigurationImportError {
    match error {
        AuthImportError::Validation(message) => ConfigurationImportError::Validation(message),
        AuthImportError::Database(error) => ConfigurationImportError::Database(error),
    }
}

struct BackendAuthImportTxPort;

#[async_trait::async_trait]
impl AuthImportTxPort for BackendAuthImportTxPort {
    fn validate_user_exports(
        &self,
        users: &[UserExport],
    ) -> std::result::Result<(), ConfigurationImportError> {
        validate_user_exports(users).map_err(map_auth_import_error)
    }

    async fn validate_users_tx(
        &self,
        tx: &mut sqlx::SqliteConnection,
        users: &[UserExport],
        include_users: bool,
        mode: ImportMode,
    ) -> std::result::Result<(), ConfigurationImportError> {
        validate_auth_import_tx(tx, users, include_users, mode)
            .await
            .map_err(map_auth_import_error)
    }

    async fn apply_auth_import_tx(
        &self,
        tx: &mut sqlx::SqliteConnection,
        users: &[UserExport],
        include_users: bool,
        mode: ImportMode,
        stats: &mut ImportStats,
    ) -> std::result::Result<(), ConfigurationImportError> {
        let auth_stats = apply_auth_import_tx(tx, users, include_users, mode)
            .await
            .map_err(map_auth_import_error)?;
        stats.users_created += auth_stats.users_created;
        stats.users_updated += auth_stats.users_updated;
        stats.users_deleted += auth_stats.users_deleted;
        Ok(())
    }
}

/// Build the transactional auth-import adapter bridging `services::config_import`
/// with `api::auth_store` inside the single import transaction.
pub(crate) fn auth_import_tx_port() -> Arc<dyn AuthImportTxPort> {
    Arc::new(BackendAuthImportTxPort)
}

struct BackendConfigBackup {
    export_service: ConfigurationExportService,
    import_service: ConfigurationImportService,
}

#[async_trait::async_trait]
impl ConfigBackupPort for BackendConfigBackup {
    async fn export_config(
        &self,
        user_exports: Vec<UserExport>,
    ) -> std::result::Result<ConfigExport, ConfigBackupError> {
        self.export_service
            .export(user_exports)
            .await
            .map_err(ConfigBackupError::Other)
    }

    async fn import_config(
        &self,
        config: ConfigExport,
        mode: ImportMode,
        on_users_committed: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> std::result::Result<ConfigBackupImportOutcome, ConfigBackupError> {
        let outcome = self
            .import_service
            .import_with_post_commit(config, mode, on_users_committed)
            .await
            .map_err(|error| match error {
                ConfigurationImportError::Validation(message) => {
                    ConfigBackupError::Validation(message)
                }
                ConfigurationImportError::MissingProxy(error) => {
                    ConfigBackupError::MissingProxy(error)
                }
                ConfigurationImportError::Database(error) => ConfigBackupError::Database(error),
            })?;
        Ok(ConfigBackupImportOutcome {
            stats: outcome.stats,
            warnings: outcome.warnings,
        })
    }
}

/// Assemble the [`ConfigBackupPort`] adapter for a core [`crate::services::ServiceContainer`].
pub(crate) fn build_config_backup(
    container: &crate::services::ServiceContainer,
) -> Arc<dyn ConfigBackupPort> {
    let export_service = ConfigurationExportService::new(
        container.write_pool.clone(),
        container.config_service.clone(),
        container.streamer_repository.clone(),
        container.filter_repository.clone(),
        container.notification_repository.clone(),
        container.job_preset_repository.clone(),
        container.pipeline_preset_repository.clone(),
    );
    let import_service = ConfigurationImportService::new(
        container.write_pool.clone(),
        container.config_service.clone(),
        container.streamer_manager.clone(),
        container.notification_service.clone(),
        container.actor_registry.clone(),
        container.platform_admission.clone(),
        auth_import_tx_port(),
    );
    Arc::new(BackendConfigBackup {
        export_service,
        import_service,
    })
}

/// Assemble the [`ApiServerRuntime`] handed to [`ApiServerHost`] from the core
/// [`crate::services::ServiceContainer`].
pub(crate) fn build_api_runtime(
    container: &crate::services::ServiceContainer,
    config_backup: Arc<dyn ConfigBackupPort>,
) -> Result<ApiServerRuntime> {
    let logging_config = container.logging_config.get().cloned().ok_or_else(|| {
        crate::Error::ApiError(
            "Logging configuration must be initialized before starting the API".to_string(),
        )
    })?;

    let services = ApiServices {
        config_service: container.config_service.clone(),
        streamer_manager: container.streamer_manager.clone(),
        pipeline_manager: container.pipeline_manager.clone(),
        download_manager: container.download_manager.clone(),
        queries: crate::api::queries::ApiQueries::new(container.pool.clone()),
        session_repository: container.session_repository.clone(),
        session_event_repository: container.session_event_repository.clone(),
        streamer_check_history_repository: container.streamer_check_history_repository.clone(),
        check_history_broadcaster: container.check_history_broadcaster.clone(),
        upload_status_broadcaster: container.upload_status_broadcaster.clone(),
        upload_record_repository: container.upload_record_repository.clone(),
        filter_repository: container.filter_repository.clone(),
        health_checker: container.health_checker.clone(),
        streamer_repository: container.streamer_repository.clone(),
        pipeline_preset_repository: container.pipeline_preset_repository.clone(),
        notification_repository: container.notification_repository.clone(),
        notification_service: container.notification_service.clone(),
        logging_config,
        logging_download_tokens: Arc::new(DashMap::new()),
        logging_archives: Arc::new(crate::api::routes::logging::LogArchiveService::new()),
        credential_service: container.credential_service.clone(),
        platform_admission: container.platform_admission.clone(),
        credential_profiles: container.credential_profiles.clone(),
        credential_execution: container.credential_execution.clone(),
        proxies: container.proxies.clone(),
        credential_blocks: container.credential_blocks().clone(),
        bigo_notifications: container.bigo_notifications.clone(),
        playback_contexts: Arc::new(PlaybackContextService::default()),
        extractor_runner: container.extractor_runner.clone(),
        danmu_taps: container.danmu_service.taps(),
        credential_login_sessions: container.credential_login_sessions.clone(),
        config_backup,
        actor_registry: container.actor_registry.clone(),
    };

    Ok(ApiServerRuntime {
        pool: container.pool.clone(),
        write_pool: container.write_pool.clone(),
        services,
        web_push_service: container.web_push_service.clone(),
        cancellation_token: container.cancellation_token.clone(),
        task_supervisor: container.task_supervisor.clone(),
    })
}

/// Configuration used to assemble a backend [`ServiceContainer`].
pub struct ServiceContainerConfig {
    pub cache_ttl: Duration,
    pub event_capacity: usize,
    pub download_config: DownloadManagerConfig,
    pub pipeline_config: PipelineManagerConfig,
    pub api_config: ApiServerConfig,
    /// Serve the API as the desktop app's built-in backend. The shell signs
    /// its user in without a password, so nobody is ever made to change the
    /// seeded default password, and `must_change_password` then means "no
    /// password has been set". With this set, an account with that flag is
    /// reachable only through sessions issued by
    /// [`ServiceContainer::issue_local_desktop_session`]: a password login for
    /// it never succeeds (the seeded password never signs in), its other
    /// sessions are refused like revoked ones, and its API keys are honoured.
    ///
    /// Part of the configuration so it is fixed before the container, and so
    /// the API server's `AuthService`, exists. Only the desktop shell's local
    /// mode sets it; the server and Docker builds never do.
    pub local_desktop_mode: bool,
}

/// Composed backend container combining core runtime services with the API
/// server host and authentication surface.
pub struct ServiceContainer {
    core: crate::services::ServiceContainer,
    config_backup: Arc<dyn ConfigBackupPort>,
    api: ApiServerHost,
}

impl Deref for ServiceContainer {
    type Target = crate::services::ServiceContainer;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl ServiceContainer {
    /// Create a new backend container with default subsystem settings and
    /// environment-configured API/auth settings.
    ///
    /// # Errors
    ///
    /// Returns an error if API/auth environment settings are invalid or the
    /// persisted global configuration cannot be loaded or initialized.
    pub async fn new(pool: SqlitePool, write_pool: SqlitePool) -> Result<Self> {
        let api_config = ApiServerConfig::from_env()?;
        Self::with_full_config(
            pool,
            write_pool,
            ServiceContainerConfig {
                cache_ttl: DEFAULT_CACHE_TTL,
                event_capacity: DEFAULT_EVENT_CAPACITY,
                download_config: DownloadManagerConfig::default(),
                pipeline_config: PipelineManagerConfig::default(),
                api_config,
                local_desktop_mode: false,
            },
        )
        .await
    }

    /// Create a backend container with custom subsystem, API, and desktop-mode
    /// settings.
    ///
    /// Database-backed global settings remain authoritative for download
    /// concurrency, pipeline worker counts, pipeline timeouts, and queue
    /// freshness.
    ///
    /// # Errors
    ///
    /// Returns an error if auth environment settings are invalid or the
    /// persisted global configuration cannot be loaded or initialized.
    pub async fn with_full_config(
        pool: SqlitePool,
        write_pool: SqlitePool,
        config: ServiceContainerConfig,
    ) -> Result<Self> {
        let ServiceContainerConfig {
            cache_ttl,
            event_capacity,
            download_config,
            pipeline_config,
            api_config,
            local_desktop_mode,
        } = config;
        // Validate auth settings before building any background service.
        let api = ApiServerHost::new(api_config, local_desktop_mode)?;
        let core = crate::services::ServiceContainer::with_full_config(
            pool,
            write_pool,
            crate::services::ServiceContainerConfig {
                cache_ttl,
                event_capacity,
                download_config,
                pipeline_config,
            },
        )
        .await?;
        let config_backup = build_config_backup(&core);
        Ok(Self {
            core,
            config_backup,
            api,
        })
    }

    /// Initialize and start the API server.
    /// This should be called after `initialize()` and runs the server in the background.
    pub async fn start_api_server(&self) -> Result<()> {
        let runtime = build_api_runtime(&self.core, self.config_backup.clone())?;
        self.api.start_api_server(runtime).await
    }

    /// Initialize and start the API server, returning the resolved bind address.
    ///
    /// This is required when binding to port `0` (ephemeral port), where the
    /// actual port is only known after binding.
    pub async fn start_api_server_bound(&self) -> Result<SocketAddr> {
        let runtime = build_api_runtime(&self.core, self.config_backup.clone())?;
        self.api.start_api_server_bound(runtime).await
    }

    /// The runtime the `start_api_server*` methods serve. The end-to-end
    /// suite serves it without authentication through
    /// `ApiServerHost::bind_and_spawn_api_server_for_tests`, because the
    /// production path reads `AUTH_DISABLED` from the process environment,
    /// which the suite must not change.
    #[doc(hidden)]
    pub fn api_runtime(&self) -> Result<ApiServerRuntime> {
        build_api_runtime(&self.core, self.config_backup.clone())
    }

    /// Initialize and start the API server, returning the resolved bind
    /// address, using a caller-provided JWT secret.
    ///
    /// This is primarily intended for the desktop (Tauri) wrapper, which
    /// should not depend on `.env` loading or shell environment setup for
    /// authentication to work.
    pub async fn start_api_server_bound_with_jwt_secret(
        &self,
        jwt_secret: String,
    ) -> Result<SocketAddr> {
        let runtime = build_api_runtime(&self.core, self.config_backup.clone())?;
        self.api
            .start_api_server_bound_with_jwt_secret(runtime, jwt_secret)
            .await
    }

    /// Issue a local desktop session for the active admin user when the
    /// loopback desktop app operates without password sign-in.
    pub async fn issue_local_desktop_session(&self) -> Result<(String, AuthResponse)> {
        self.api.issue_local_desktop_session().await
    }

    /// The account local desktop sessions are issued for, or `None` when
    /// there is no active admin.
    pub async fn local_desktop_account(&self) -> Result<Option<LocalDesktopAccount>> {
        self.api.local_desktop_account().await
    }

    /// Replace the initial password of the local desktop account. Fails once
    /// that account has a password of its own.
    pub async fn set_local_desktop_password(&self, new_password: &str) -> Result<()> {
        self.api.set_local_desktop_password(new_password).await
    }

    /// Restore previously persisted local desktop `(session_id, user_id)` pairs
    /// on startup when password sign-in remains disabled.
    pub fn restore_local_desktop_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String)>,
    ) {
        self.api.restore_local_desktop_sessions(sessions);
    }

    /// Return the currently tracked local desktop `(session_id, user_id)` pairs.
    pub fn list_local_desktop_sessions(&self) -> Vec<(String, String)> {
        self.api.list_local_desktop_sessions()
    }

    /// Revoke all tracked local-desktop sessions (plus any persisted entries) in
    /// SQLite and clear their in-memory exemptions.
    pub async fn revoke_local_desktop_sessions(
        &self,
        extra_sessions: &[(String, String)],
    ) -> Result<usize> {
        self.api.revoke_local_desktop_sessions(extra_sessions).await
    }
}

/// The HTTP client for the desktop app's check for a newer release, chosen
/// from the global route of `container` as it is now. Callers set timeouts
/// per request.
///
/// A global route naming a saved proxy takes that proxy, through the shared
/// client; a saved proxy that no longer exists fails rather than connecting
/// another way. Every other case takes the system proxy: a global route of
/// Direct or System proxy, and no `container` (a remote-mode desktop, or a
/// built-in backend that failed before its services existed). The check
/// contacts GitHub for the app itself rather than for the recorder, and a new
/// installation's Direct records only that no proxy environment variable was
/// set on its first start, not that the operating system's proxy should be
/// bypassed.
///
/// The system-proxy client is built for each check rather than shared:
/// reqwest reads the environment and the operating system's proxy setting
/// when a client is built, so a shared client would keep using the proxy in
/// effect at its first use even after the user starts or stops a local proxy.
pub async fn release_check_client(
    container: Option<&ServiceContainer>,
) -> crate::Result<reqwest::Client> {
    use crate::proxies::{ProxyRoute, ProxyTarget};
    use crate::utils::http_client::{apply_proxy, build_client, service_client};

    let target = match container {
        Some(container) => {
            container
                .proxies
                .routes()
                .target(&ProxyRoute::Inherit)
                .await?
        }
        None => ProxyTarget::System,
    };
    match target {
        ProxyTarget::Explicit(_) => Ok(service_client(&target)?),
        ProxyTarget::Direct | ProxyTarget::System => Ok(build_client(apply_proxy(
            reqwest::Client::builder(),
            &ProxyTarget::System,
        ))?),
    }
}
