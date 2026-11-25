//! API server bootstrapping, authentication mode validation, and local desktop
//! session management hosted on top of [`crate::services::ServiceContainer`].

use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{Arc, OnceLock},
};

use dashmap::DashMap;
use tracing::{info, warn};

use crate::Result;
use crate::api::auth_service::{AuthConfig, AuthResponse, AuthService, LocalDesktopAccount};
use crate::api::auth_store::{
    SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
};
use crate::api::backup::ConfigBackupPort;
use crate::api::jwt::JwtService;
use crate::api::playback_context::PlaybackContextService;
use crate::api::server::{ApiServer, ApiServerConfig, ApiServices, AppState};
use crate::services::ServiceContainer;

#[cfg(test)]
mod import_contract_tests;
#[cfg(test)]
mod response_contract_tests;
#[cfg(test)]
mod timezone_import_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiAuthMode {
    Enabled,
    Disabled,
}

fn parse_auth_disabled(value: Option<&str>) -> Result<bool> {
    let Some(value) = value else {
        return Ok(false);
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(crate::Error::config(format!(
            "Invalid AUTH_DISABLED value '{value}'; expected true, false, 1, or 0"
        ))),
    }
}

fn validate_auth_mode(
    jwt_configured: bool,
    auth_disabled: bool,
    bind_address: &str,
) -> Result<ApiAuthMode> {
    if jwt_configured {
        return Ok(ApiAuthMode::Enabled);
    }

    if !auth_disabled {
        return Err(crate::Error::config(
            "JWT_SECRET is required; for local development only, set AUTH_DISABLED=true and bind to a loopback IP",
        ));
    }

    let bind_ip = IpAddr::from_str(bind_address.trim()).map_err(|_| {
        crate::Error::config(
            "AUTH_DISABLED=true requires API_BIND_ADDRESS to be a loopback IP address",
        )
    })?;
    if !bind_ip.is_loopback() {
        return Err(crate::Error::config(format!(
            "AUTH_DISABLED=true is not allowed for non-loopback bind address '{bind_address}'"
        )));
    }

    Ok(ApiAuthMode::Disabled)
}

/// Assemble the Axum [`AppState`] from a core [`ServiceContainer`], a
/// [`ConfigBackupPort`], and an optional [`AuthService`], installing the API
/// layer's WebSocket Protobuf encoders on the shared broadcasters.
pub(crate) fn build_api_state(
    container: &ServiceContainer,
    config_backup: Arc<dyn ConfigBackupPort>,
    auth_service: Option<Arc<AuthService>>,
) -> Result<AppState> {
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

    let mut state = AppState::new(services);
    if let Some(auth_service) = auth_service {
        state = state.with_auth_service(auth_service);
    }
    if let Some(web_push_service) = container.web_push_service.clone() {
        state = state.with_web_push_service(web_push_service);
    }

    Ok(state)
}

/// API-layer host state owned by the top-level backend facade.
pub(crate) struct ApiServerHost {
    api_server_config: ApiServerConfig,
    /// Token and password settings, validated up front so an invalid value
    /// fails startup before any service starts.
    auth_config: AuthConfig,
    /// Installed when the API server starts with database authentication.
    auth_service: OnceLock<Arc<AuthService>>,
    /// Serve the API in local desktop mode (in-process auto-login sessions).
    local_desktop_mode: bool,
}

impl ApiServerHost {
    pub(crate) fn new(
        api_server_config: ApiServerConfig,
        local_desktop_mode: bool,
    ) -> Result<Self> {
        let auth_config = AuthConfig::from_env()?;
        Ok(Self {
            api_server_config,
            auth_config,
            auth_service: OnceLock::new(),
            local_desktop_mode,
        })
    }

    /// Build `AppState`, bridge the container's cancellation token to the
    /// server's, bind the listener, and hand the accept loop to the task
    /// supervisor as a critical task.
    ///
    /// Shared tail of [`Self::start_api_server_bound`] and
    /// [`Self::start_api_server_bound_with_jwt_secret`]; `bridge_task` and
    /// `server_task` keep the two call paths distinguishable in supervisor
    /// logs.
    async fn bind_and_spawn_api_server(
        &self,
        container: &ServiceContainer,
        config_backup: Arc<dyn ConfigBackupPort>,
        auth_service: Option<Arc<AuthService>>,
        bridge_task: &'static str,
        server_task: &'static str,
    ) -> Result<SocketAddr> {
        let state = build_api_state(container, config_backup, auth_service.clone())?;
        if let Some(auth_service) = auth_service
            && self.auth_service.set(auth_service).is_err()
        {
            warn!("API server started more than once; local desktop sessions use the first one");
        }
        let server = ApiServer::new(self.api_server_config.clone(), state);
        let cancel_token = container.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        container.task_supervisor.spawn(bridge_task, async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        if !container
            .task_supervisor
            .spawn_critical(server_task, async move {
                server.run_with_listener(listener).await
            })
        {
            return Err(crate::Error::Other(format!(
                "{server_task} task was rejected during shutdown"
            )));
        }

        Ok(local_addr)
    }

    /// Build the user/token/API-key backed authentication service.
    fn database_auth_service(
        &self,
        container: &ServiceContainer,
        jwt: Arc<JwtService>,
        auth_config: AuthConfig,
    ) -> AuthService {
        let service = AuthService::new(
            Arc::new(SqlxUserRepository::new(
                container.pool.clone(),
                container.write_pool.clone(),
            )),
            Arc::new(SqlxRefreshTokenRepository::new(
                container.pool.clone(),
                container.write_pool.clone(),
            )),
            Arc::new(SqlxApiKeyRepository::new(
                container.pool.clone(),
                container.write_pool.clone(),
            )),
            jwt,
            auth_config,
        );
        if self.local_desktop_mode {
            service.with_local_desktop_mode()
        } else {
            service
        }
    }

    /// Initialize and start the API server.
    pub(crate) async fn start_api_server(
        &self,
        container: &ServiceContainer,
        config_backup: Arc<dyn ConfigBackupPort>,
    ) -> Result<()> {
        self.start_api_server_bound(container, config_backup)
            .await?;
        Ok(())
    }

    /// Initialize and start the API server, returning the resolved bind address.
    pub(crate) async fn start_api_server_bound(
        &self,
        container: &ServiceContainer,
        config_backup: Arc<dyn ConfigBackupPort>,
    ) -> Result<SocketAddr> {
        let auth_config = self.auth_config.clone();

        let jwt_service =
            JwtService::from_env(auth_config.access_token_expiration_secs).map(Arc::new);
        let auth_disabled_value = std::env::var("AUTH_DISABLED").ok();
        let auth_disabled = parse_auth_disabled(auth_disabled_value.as_deref())?;
        let auth_mode = validate_auth_mode(
            jwt_service.is_some(),
            auth_disabled,
            &self.api_server_config.bind_address,
        )?;

        let auth_service = match (auth_mode, jwt_service) {
            (ApiAuthMode::Enabled, Some(jwt)) => {
                let auth_svc = self.database_auth_service(container, jwt, auth_config);
                info!("AuthService initialized with user database authentication");
                Some(Arc::new(auth_svc))
            }
            (ApiAuthMode::Disabled, None) => {
                warn!(
                    bind_address = %self.api_server_config.bind_address,
                    "AUTHENTICATION DISABLED: all API endpoints are unauthenticated"
                );
                None
            }
            _ => {
                return Err(crate::Error::config(
                    "Authentication configuration resolved to an inconsistent state",
                ));
            }
        };

        self.bind_and_spawn_api_server(
            container,
            config_backup,
            auth_service,
            "API shutdown bridge",
            "API server",
        )
        .await
    }

    /// Initialize and start the API server, returning the resolved bind address, using a
    /// caller-provided JWT secret.
    pub(crate) async fn start_api_server_bound_with_jwt_secret(
        &self,
        container: &ServiceContainer,
        config_backup: Arc<dyn ConfigBackupPort>,
        jwt_secret: String,
    ) -> Result<SocketAddr> {
        let auth_config = self.auth_config.clone();

        let (issuer, audience) = crate::api::jwt::issuer_and_audience_from_env();
        let jwt_service = Arc::new(
            JwtService::new(
                &jwt_secret,
                &issuer,
                &audience,
                Some(auth_config.access_token_expiration_secs),
            )
            .map_err(|e| crate::Error::Configuration(e.to_string()))?,
        );

        let auth_svc = self.database_auth_service(container, jwt_service, auth_config);
        info!(
            issuer = %issuer,
            audience = %audience,
            "AuthService initialized with desktop-provided JWT secret"
        );

        self.bind_and_spawn_api_server(
            container,
            config_backup,
            Some(Arc::new(auth_svc)),
            "desktop API shutdown bridge",
            "desktop API server",
        )
        .await
    }

    pub(crate) async fn issue_local_desktop_session(&self) -> Result<(String, AuthResponse)> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.issue_local_desktop_session()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub(crate) async fn local_desktop_account(&self) -> Result<Option<LocalDesktopAccount>> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.local_desktop_account()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub(crate) async fn set_local_desktop_password(&self, new_password: &str) -> Result<()> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.set_local_desktop_password(new_password)
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub(crate) fn restore_local_desktop_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String)>,
    ) {
        if let Some(auth) = self.auth_service.get() {
            auth.restore_local_desktop_sessions(sessions);
        }
    }

    pub(crate) fn list_local_desktop_sessions(&self) -> Vec<(String, String)> {
        self.auth_service
            .get()
            .map(|auth| auth.list_local_desktop_sessions())
            .unwrap_or_default()
    }

    pub(crate) async fn revoke_local_desktop_sessions(
        &self,
        extra_sessions: &[(String, String)],
    ) -> Result<usize> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.revoke_local_desktop_sessions(extra_sessions)
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_auth_disabled_accepts_explicit_boolean_values() {
        assert!(!parse_auth_disabled(None).expect("missing value should use the default"));
        assert!(parse_auth_disabled(Some("true")).expect("true should parse"));
        assert!(parse_auth_disabled(Some("1")).expect("1 should parse"));
        assert!(!parse_auth_disabled(Some("FALSE")).expect("false should parse"));
        assert!(!parse_auth_disabled(Some("0")).expect("0 should parse"));
    }

    #[test]
    fn parse_auth_disabled_rejects_invalid_values() {
        let result = parse_auth_disabled(Some("yes"));
        assert!(matches!(result, Err(crate::Error::Configuration(_))));
    }

    #[test]
    fn missing_jwt_fails_closed_by_default() {
        let result = validate_auth_mode(false, false, "127.0.0.1");
        assert!(matches!(result, Err(crate::Error::Configuration(_))));
    }

    #[test]
    fn explicit_auth_opt_out_is_limited_to_loopback() {
        assert_eq!(
            validate_auth_mode(false, true, "127.0.0.1")
                .expect("IPv4 loopback should allow the explicit opt-out"),
            ApiAuthMode::Disabled
        );
        assert_eq!(
            validate_auth_mode(false, true, "::1")
                .expect("IPv6 loopback should allow the explicit opt-out"),
            ApiAuthMode::Disabled
        );

        for bind_address in ["0.0.0.0", "192.168.1.10", "::", "localhost"] {
            let result = validate_auth_mode(false, true, bind_address);
            assert!(
                matches!(result, Err(crate::Error::Configuration(_))),
                "{bind_address} must not allow unauthenticated startup"
            );
        }
    }

    /// The served API keys failed logins by the connection's peer address.
    /// The router must be served with its connect info for `ClientIp` to see
    /// that address; without it the per-address budget silently never
    /// applies and one client can spread Argon2 work over any number of
    /// usernames. An address budget of one makes the second failure, under
    /// another username, the throttled one. The end-to-end suite cannot
    /// reach this: the default budget is 100 failures, each a slow Argon2
    /// verification.
    #[tokio::test]
    async fn the_served_api_budgets_failed_logins_per_peer_address() {
        use std::time::Duration;

        use crate::api::rate_limit::LoginRateLimiter;

        let directory = tempfile::tempdir().unwrap();
        let (container, host, pool) = served_test_container(directory.path(), false).await;
        let auth = AuthService::new(
            Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool)),
            Arc::new(
                JwtService::new(
                    "client-ip-test-secret-with-enough-entropy",
                    "rust-srec",
                    "rust-srec-api",
                    None,
                )
                .unwrap(),
            ),
            AuthConfig::default(),
        )
        .with_login_rate_limiter(LoginRateLimiter::new(5, 1, Duration::from_secs(60), 8));
        let address = host
            .bind_and_spawn_api_server(
                &container,
                crate::backend::build_config_backup(&container),
                Some(Arc::new(auth)),
                "test bridge",
                "test server",
            )
            .await
            .unwrap();

        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        let mut statuses = Vec::new();
        for username in ["alice", "bob"] {
            let response = client
                .post(format!("http://{address}/api/auth/login"))
                .json(&serde_json::json!({ "username": username, "password": "wrong-password-1" }))
                .send()
                .await
                .unwrap();
            statuses.push(response.status());
        }
        assert_eq!(
            statuses,
            [
                reqwest::StatusCode::UNAUTHORIZED,
                reqwest::StatusCode::TOO_MANY_REQUESTS
            ]
        );
        container.cancellation_token.cancel();
    }

    /// The desktop shell builds its container with local desktop mode on.
    /// The served login must then answer the seeded `admin` / `admin123!`
    /// exactly like a wrong password, which only holds if the container
    /// hands the mode to the `AuthService` it builds.
    #[tokio::test]
    async fn local_desktop_mode_reaches_the_served_login() {
        let directory = tempfile::tempdir().unwrap();
        let (container, host, _pool) = served_test_container(directory.path(), true).await;
        let address = host
            .start_api_server_bound_with_jwt_secret(
                &container,
                crate::backend::build_config_backup(&container),
                "local-desktop-mode-test-secret-with-entropy".to_owned(),
            )
            .await
            .unwrap();

        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        let mut answers = Vec::new();
        for password in ["admin123!", "wrong-password-1"] {
            let response = client
                .post(format!("http://{address}/api/auth/login"))
                .json(&serde_json::json!({ "username": "admin", "password": password }))
                .send()
                .await
                .unwrap();
            answers.push((response.status(), response.text().await.unwrap()));
        }
        assert_eq!(answers[0].0, reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(answers[0], answers[1]);
        container.cancellation_token.cancel();
    }

    /// A container and API host on a fresh in-memory database, ready to start
    /// the API on an ephemeral loopback port.
    async fn served_test_container(
        log_dir: &std::path::Path,
        local_desktop_mode: bool,
    ) -> (ServiceContainer, ApiServerHost, sqlx::SqlitePool) {
        use std::time::Duration;

        use crate::services::ServiceContainerConfig;

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let container = ServiceContainer::with_full_config(
            pool.clone(),
            pool.clone(),
            ServiceContainerConfig {
                cache_ttl: Duration::from_secs(60),
                event_capacity: 8,
                download_config: crate::downloader::DownloadManagerConfig::default(),
                pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
            },
        )
        .await
        .unwrap();
        let host = ApiServerHost::new(
            ApiServerConfig {
                bind_address: "127.0.0.1".to_owned(),
                port: 0,
                enable_cors: false,
                body_limit: 1024 * 1024,
            },
            local_desktop_mode,
        )
        .unwrap();
        let (logging, _layer) =
            crate::logging::LoggingConfig::for_route_tests(log_dir.to_path_buf());
        assert!(container.logging_config.set(Arc::new(logging)).is_ok());
        (container, host, pool)
    }
}
