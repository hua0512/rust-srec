use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info};

use crate::Result;
use crate::api::auth_service::{AuthConfig, AuthService};
use crate::api::{
    ApiServer, JwtService,
    server::{ApiServices, AppState},
};
use crate::database::repositories::{
    SqlxRefreshTokenRepository, SqlxUserRepository,
    filter::SqlxFilterRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    streamer::SqlxStreamerRepository,
};

use super::ServiceContainer;

impl ServiceContainer {
    /// Initialize and start the API server.
    /// This should be called after initialize() and runs the server in the background.
    pub async fn start_api_server(&self) -> Result<()> {
        let _ = self.start_api_server_bound().await?;
        Ok(())
    }

    fn build_api_state(
        &self,
        jwt_service: Option<Arc<JwtService>>,
        auth_service: Option<Arc<AuthService>>,
    ) -> Result<AppState> {
        let logging_config = self.logging_config.get().cloned().ok_or_else(|| {
            crate::Error::ApiError(
                "Logging configuration must be initialized before starting the API".to_string(),
            )
        })?;

        let services = ApiServices {
            config_service: self.config_service.clone(),
            streamer_manager: self.streamer_manager.clone(),
            pipeline_manager: self.pipeline_manager.clone(),
            download_manager: self.download_manager.clone(),
            session_repository: self.session_repository.clone(),
            session_event_repository: Arc::new(
                crate::database::repositories::SqlxSessionEventRepository::new(
                    self.pool.clone(),
                    self.write_pool.clone(),
                ),
            ),
            streamer_check_history_repository: Arc::new(
                crate::database::repositories::SqlxStreamerCheckHistoryRepository::new(
                    self.pool.clone(),
                    self.write_pool.clone(),
                ),
            ),
            check_history_broadcaster: self.check_history_broadcaster.clone(),
            filter_repository: Arc::new(SqlxFilterRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )),
            health_checker: self.health_checker.clone(),
            streamer_repository: Arc::new(SqlxStreamerRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )),
            pipeline_preset_repository: Arc::new(SqlitePipelinePresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )),
            job_preset_repository: Arc::new(SqliteJobPresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )),
            notification_repository: self.notification_repository.clone(),
            notification_service: self.notification_service.clone(),
            logging_config,
            logging_download_tokens: Arc::new(DashMap::new()),
            credential_service: self.credential_service.clone(),
        };

        let mut state = AppState::new(services);
        if let Some(jwt_service) = jwt_service {
            state = state.with_jwt_service(jwt_service);
        }
        if let Some(auth_service) = auth_service {
            state = state.with_auth_service(auth_service);
        }
        if let Some(web_push_service) = self.web_push_service.clone() {
            state = state.with_web_push_service(web_push_service);
        }

        Ok(state)
    }

    /// Initialize and start the API server, returning the resolved bind address.
    ///
    /// This is required when binding to port `0` (ephemeral port), where the actual port is only
    /// known after binding.
    pub async fn start_api_server_bound(&self) -> Result<std::net::SocketAddr> {
        // Create AuthConfig from environment first (single source of truth for token expiration)
        let auth_config = AuthConfig::from_env();

        let jwt_service =
            JwtService::from_env(auth_config.access_token_expiration_secs).map(Arc::new);

        // Create AuthService if JWT is configured
        let auth_service = if let Some(ref jwt) = jwt_service {
            // Create user and refresh token repositories
            let user_repo = Arc::new(SqlxUserRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            ));
            let token_repo = Arc::new(SqlxRefreshTokenRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            ));

            let auth_svc = AuthService::new(user_repo, token_repo, jwt.clone(), auth_config);
            info!("AuthService initialized with user database authentication");
            Some(Arc::new(auth_svc))
        } else {
            debug!("JWT not configured, AuthService disabled");
            None
        };

        let state = self.build_api_state(jwt_service, auth_service)?;
        let server = ApiServer::new(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        tokio::spawn(async move {
            if let Err(e) = server.run_with_listener(listener).await {
                tracing::error!("API server error: {}", e);
            }
        });

        Ok(local_addr)
    }

    /// Initialize and start the API server, returning the resolved bind address, using a
    /// caller-provided JWT secret.
    ///
    /// This is primarily intended for the desktop (Tauri) wrapper, which should not depend on
    /// `.env` loading / shell environment setup for authentication to work.
    pub async fn start_api_server_bound_with_jwt_secret(
        &self,
        jwt_secret: String,
    ) -> Result<std::net::SocketAddr> {
        let auth_config = AuthConfig::from_env();

        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "rust-srec".to_string());
        let audience =
            std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "rust-srec-api".to_string());
        let jwt_service = Arc::new(JwtService::new(
            &jwt_secret,
            &issuer,
            &audience,
            Some(auth_config.access_token_expiration_secs),
        ));

        // Create AuthService (always enabled when a JWT secret is provided)
        let user_repo = Arc::new(SqlxUserRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        ));
        let token_repo = Arc::new(SqlxRefreshTokenRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        ));
        let auth_svc = AuthService::new(user_repo, token_repo, jwt_service.clone(), auth_config);
        info!(
            issuer = %issuer,
            audience = %audience,
            "AuthService initialized with desktop-provided JWT secret"
        );

        let state = self.build_api_state(Some(jwt_service), Some(Arc::new(auth_svc)))?;
        let server = ApiServer::new(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        tokio::spawn(async move {
            if let Err(e) = server.run_with_listener(listener).await {
                tracing::error!("API server error: {}", e);
            }
        });

        Ok(local_addr)
    }
}
