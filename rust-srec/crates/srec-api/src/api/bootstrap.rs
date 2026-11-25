//! API server bootstrapping, authentication mode validation, and local desktop
//! session management.

use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{Arc, OnceLock},
};

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Result;
use crate::api::auth_service::{AuthConfig, AuthResponse, AuthService, LocalDesktopAccount};
use crate::api::auth_store::{
    SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
};
use crate::api::jwt::JwtService;
use crate::api::server::{ApiServer, ApiServerConfig, ApiServices, AppState};
use crate::notification::web_push::WebPushService;
use crate::utils::task_supervisor::TaskSupervisor;

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

/// Runtime dependencies injected into [`ApiServerHost`] by the application
/// composition shell.
pub struct ApiServerRuntime {
    pub pool: SqlitePool,
    pub write_pool: SqlitePool,
    pub services: ApiServices,
    pub web_push_service: Option<Arc<WebPushService>>,
    pub cancellation_token: CancellationToken,
    pub task_supervisor: Arc<TaskSupervisor>,
}

/// Assemble the Axum [`AppState`] from [`ApiServerRuntime`] and an optional
/// [`AuthService`], installing the API layer's WebSocket Protobuf encoders on
/// the shared broadcasters.
pub fn build_api_state(
    runtime: ApiServerRuntime,
    auth_service: Option<Arc<AuthService>>,
) -> AppState {
    let mut state = AppState::new(runtime.services);
    if let Some(auth_service) = auth_service {
        state = state.with_auth_service(auth_service);
    }
    if let Some(web_push_service) = runtime.web_push_service {
        state = state.with_web_push_service(web_push_service);
    }
    state
}

/// API-layer host state owned by the top-level backend facade.
pub struct ApiServerHost {
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
    pub fn new(api_server_config: ApiServerConfig, local_desktop_mode: bool) -> Result<Self> {
        let auth_config = AuthConfig::from_env()?;
        Ok(Self {
            api_server_config,
            auth_config,
            auth_service: OnceLock::new(),
            local_desktop_mode,
        })
    }

    /// Test helper to bind and spawn the API server with a custom [`AuthService`].
    #[cfg(any(test, feature = "test-support"))]
    pub async fn bind_and_spawn_api_server_for_tests(
        &self,
        runtime: ApiServerRuntime,
        auth_service: Option<Arc<AuthService>>,
        bridge_task: &'static str,
        server_task: &'static str,
    ) -> Result<SocketAddr> {
        self.bind_and_spawn_api_server(runtime, auth_service, bridge_task, server_task)
            .await
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
        runtime: ApiServerRuntime,
        auth_service: Option<Arc<AuthService>>,
        bridge_task: &'static str,
        server_task: &'static str,
    ) -> Result<SocketAddr> {
        let cancel_token = runtime.cancellation_token.clone();
        let task_supervisor = runtime.task_supervisor.clone();
        let state = build_api_state(runtime, auth_service.clone());
        if let Some(auth_service) = auth_service
            && self.auth_service.set(auth_service).is_err()
        {
            warn!("API server started more than once; local desktop sessions use the first one");
        }
        let server = ApiServer::new(self.api_server_config.clone(), state);

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        task_supervisor.spawn(bridge_task, async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        if !task_supervisor.spawn_critical(server_task, async move {
            server.run_with_listener(listener).await
        }) {
            return Err(crate::Error::Other(format!(
                "{server_task} task was rejected during shutdown"
            )));
        }

        Ok(local_addr)
    }

    /// Build the user/token/API-key backed authentication service.
    fn database_auth_service(
        &self,
        pool: &SqlitePool,
        write_pool: &SqlitePool,
        jwt: Arc<JwtService>,
        auth_config: AuthConfig,
    ) -> AuthService {
        let service = AuthService::new(
            Arc::new(SqlxUserRepository::new(pool.clone(), write_pool.clone())),
            Arc::new(SqlxRefreshTokenRepository::new(
                pool.clone(),
                write_pool.clone(),
            )),
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), write_pool.clone())),
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
    pub async fn start_api_server(&self, runtime: ApiServerRuntime) -> Result<()> {
        self.start_api_server_bound(runtime).await?;
        Ok(())
    }

    /// Initialize and start the API server, returning the resolved bind address.
    pub async fn start_api_server_bound(&self, runtime: ApiServerRuntime) -> Result<SocketAddr> {
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
                let auth_svc = self.database_auth_service(
                    &runtime.pool,
                    &runtime.write_pool,
                    jwt,
                    auth_config,
                );
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

        self.bind_and_spawn_api_server(runtime, auth_service, "API shutdown bridge", "API server")
            .await
    }

    /// Initialize and start the API server, returning the resolved bind address, using a
    /// caller-provided JWT secret.
    pub async fn start_api_server_bound_with_jwt_secret(
        &self,
        runtime: ApiServerRuntime,
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

        let auth_svc = self.database_auth_service(
            &runtime.pool,
            &runtime.write_pool,
            jwt_service,
            auth_config,
        );
        info!(
            issuer = %issuer,
            audience = %audience,
            "AuthService initialized with desktop-provided JWT secret"
        );

        self.bind_and_spawn_api_server(
            runtime,
            Some(Arc::new(auth_svc)),
            "desktop API shutdown bridge",
            "desktop API server",
        )
        .await
    }

    pub async fn issue_local_desktop_session(&self) -> Result<(String, AuthResponse)> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.issue_local_desktop_session()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub async fn local_desktop_account(&self) -> Result<Option<LocalDesktopAccount>> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.local_desktop_account()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub async fn set_local_desktop_password(&self, new_password: &str) -> Result<()> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.set_local_desktop_password(new_password)
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    pub fn restore_local_desktop_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String)>,
    ) {
        if let Some(auth) = self.auth_service.get() {
            auth.restore_local_desktop_sessions(sessions);
        }
    }

    pub fn list_local_desktop_sessions(&self) -> Vec<(String, String)> {
        self.auth_service
            .get()
            .map(|auth| auth.list_local_desktop_sessions())
            .unwrap_or_default()
    }

    pub async fn revoke_local_desktop_sessions(
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
}
