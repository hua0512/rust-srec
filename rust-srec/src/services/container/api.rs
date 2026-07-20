use std::{net::IpAddr, str::FromStr, sync::Arc};

use dashmap::DashMap;
use tracing::{info, warn};

use crate::Result;
use crate::api::auth_service::{AuthConfig, AuthService};
use crate::api::jwt::JwtService;
use crate::api::server::{ApiServer, ApiServices, AppState};
use crate::database::repositories::{
    SqlxRefreshTokenRepository, SqlxUserRepository,
    filter::SqlxFilterRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    streamer::SqlxStreamerRepository,
};

use super::ServiceContainer;

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

impl ServiceContainer {
    /// Initialize and start the API server.
    /// This should be called after initialize() and runs the server in the background.
    pub async fn start_api_server(&self) -> Result<()> {
        let _ = self.start_api_server_bound().await?;
        Ok(())
    }

    fn build_api_state(&self, auth_service: Option<Arc<AuthService>>) -> Result<AppState> {
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
        let auth_disabled_value = std::env::var("AUTH_DISABLED").ok();
        let auth_disabled = parse_auth_disabled(auth_disabled_value.as_deref())?;
        let auth_mode = validate_auth_mode(
            jwt_service.is_some(),
            auth_disabled,
            &self.api_server_config.bind_address,
        )?;

        let auth_service = match (auth_mode, jwt_service) {
            (ApiAuthMode::Enabled, Some(jwt)) => {
                let user_repo = Arc::new(SqlxUserRepository::new(
                    self.pool.clone(),
                    self.write_pool.clone(),
                ));
                let token_repo = Arc::new(SqlxRefreshTokenRepository::new(
                    self.pool.clone(),
                    self.write_pool.clone(),
                ));

                let auth_svc = AuthService::new(user_repo, token_repo, jwt, auth_config);
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

        let state = self.build_api_state(auth_service)?;
        let server = ApiServer::new(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        self.task_supervisor
            .spawn("API shutdown bridge", async move {
                cancel_token.cancelled().await;
                server_cancel.cancel();
            });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        self.task_supervisor.spawn("API server", async move {
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

        let state = self.build_api_state(Some(Arc::new(auth_svc)))?;
        let server = ApiServer::new(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        self.task_supervisor
            .spawn("desktop API shutdown bridge", async move {
                cancel_token.cancelled().await;
                server_cancel.cancel();
            });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        self.task_supervisor
            .spawn("desktop API server", async move {
                if let Err(e) = server.run_with_listener(listener).await {
                    tracing::error!("API server error: {}", e);
                }
            });

        Ok(local_addr)
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
    fn configured_jwt_enables_authentication() {
        assert_eq!(
            validate_auth_mode(true, false, "0.0.0.0")
                .expect("a configured JWT should enable authentication"),
            ApiAuthMode::Enabled
        );
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
}
