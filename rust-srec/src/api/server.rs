//! API server setup and configuration.

use axum::Router;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::api::routes;
use crate::error::Result;

/// API server configuration.
#[derive(Debug, Clone)]
pub struct ApiServerConfig {
    /// Server bind address
    pub bind_address: String,
    /// Server port
    pub port: u16,
    /// Enable CORS
    pub enable_cors: bool,
    /// Request body size limit in bytes
    pub body_limit: usize,
}

impl Default for ApiServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".to_string(),
            port: 12555,
            enable_cors: true,
            body_limit: 10 * 1024 * 1024, // 10MB
        }
    }
}

use std::sync::Arc;

use crate::api::auth_service::AuthService;
use crate::api::jwt::JwtService;
use crate::config::ConfigService;
use crate::danmu::DanmuService;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::FilterRepository, session::SessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::DownloadManager;
use crate::metrics::HealthChecker;
use crate::pipeline::PipelineManager;
use crate::streamer::StreamerManager;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Server start time for uptime calculation
    pub start_time: Instant,
    /// JWT service for authentication
    pub jwt_service: Option<Arc<JwtService>>,
    /// Auth service for user authentication and token management
    pub auth_service: Option<Arc<AuthService>>,
    /// Configuration service
    pub config_service: Option<Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>>,
    /// Streamer manager
    pub streamer_manager: Option<Arc<StreamerManager<SqlxStreamerRepository>>>,
    /// Pipeline manager
    pub pipeline_manager: Option<Arc<PipelineManager>>,
    /// Danmu service
    pub danmu_service: Option<Arc<DanmuService>>,
    /// Download manager
    pub download_manager: Option<Arc<DownloadManager>>,
    /// Session repository for session and output queries
    pub session_repository: Option<Arc<dyn SessionRepository>>,
    /// Filter repository for streamer filters
    pub filter_repository: Option<Arc<dyn FilterRepository>>,
    /// Health checker for real health status
    pub health_checker: Option<Arc<HealthChecker>>,
}

impl AppState {
    /// Create a new application state without services (for testing).
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            jwt_service: None,
            auth_service: None,
            config_service: None,
            streamer_manager: None,
            pipeline_manager: None,
            danmu_service: None,
            download_manager: None,
            session_repository: None,
            filter_repository: None,
            health_checker: None,
        }
    }

    /// Create a new application state with JWT service from environment variables.
    pub fn with_jwt_from_env() -> Self {
        let jwt_service = Self::create_jwt_service_from_env();
        Self {
            start_time: Instant::now(),
            jwt_service,
            auth_service: None,
            config_service: None,
            streamer_manager: None,
            pipeline_manager: None,
            danmu_service: None,
            download_manager: None,
            session_repository: None,
            filter_repository: None,
            health_checker: None,
        }
    }

    /// Create JWT service from environment variables.
    fn create_jwt_service_from_env() -> Option<Arc<JwtService>> {
        let secret = std::env::var("JWT_SECRET").ok()?;
        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "rust-srec".to_string());
        let audience =
            std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "rust-srec-api".to_string());
        let expiration_secs = std::env::var("JWT_EXPIRATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        Some(Arc::new(JwtService::new(
            &secret,
            &issuer,
            &audience,
            Some(expiration_secs),
        )))
    }

    /// Create application state with all services.
    pub fn with_services(
        jwt_service: Option<Arc<JwtService>>,
        config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
        pipeline_manager: Arc<PipelineManager>,
        danmu_service: Arc<DanmuService>,
        download_manager: Arc<DownloadManager>,
    ) -> Self {
        Self {
            start_time: Instant::now(),
            jwt_service,
            auth_service: None,
            config_service: Some(config_service),
            streamer_manager: Some(streamer_manager),
            pipeline_manager: Some(pipeline_manager),
            danmu_service: Some(danmu_service),
            download_manager: Some(download_manager),
            session_repository: None,
            filter_repository: None,
            health_checker: None,
        }
    }

    /// Set the session repository.
    pub fn with_session_repository(
        mut self,
        session_repository: Arc<dyn SessionRepository>,
    ) -> Self {
        self.session_repository = Some(session_repository);
        self
    }

    /// Set the filter repository.
    pub fn with_filter_repository(mut self, filter_repository: Arc<dyn FilterRepository>) -> Self {
        self.filter_repository = Some(filter_repository);
        self
    }

    /// Set the JWT service.
    pub fn with_jwt_service(mut self, jwt_service: Arc<JwtService>) -> Self {
        self.jwt_service = Some(jwt_service);
        self
    }

    /// Set the auth service.
    pub fn with_auth_service(mut self, auth_service: Arc<AuthService>) -> Self {
        self.auth_service = Some(auth_service);
        self
    }

    /// Set the health checker.
    pub fn with_health_checker(mut self, health_checker: Arc<HealthChecker>) -> Self {
        self.health_checker = Some(health_checker);
        self
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// API server.
pub struct ApiServer {
    config: ApiServerConfig,
    state: AppState,
    cancel_token: CancellationToken,
}

impl ApiServer {
    /// Create a new API server.
    pub fn new(config: ApiServerConfig) -> Self {
        Self {
            config,
            state: AppState::new(),
            cancel_token: CancellationToken::new(),
        }
    }

    /// Create with custom state.
    pub fn with_state(config: ApiServerConfig, state: AppState) -> Self {
        Self {
            config,
            state,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Get the cancellation token for graceful shutdown.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Build the router with all middleware and routes.
    fn build_router(&self) -> Router {
        let mut router = routes::create_router(self.state.clone());

        // Add CORS if enabled
        if self.config.enable_cors {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any);
            router = router.layer(cors);
        }

        // Add tracing
        router = router.layer(TraceLayer::new_for_http());
        router
    }

    /// Start the server.
    pub async fn run(&self) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.bind_address, self.config.port)
            .parse()
            .map_err(|e| crate::error::Error::ApiError(format!("Invalid address: {}", e)))?;

        let router = self.build_router();
        let listener = TcpListener::bind(addr).await?;

        tracing::info!("API server listening on http://{}", addr);

        let cancel_token = self.cancel_token.clone();

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                cancel_token.cancelled().await;
                tracing::info!("API server shutting down...");
            })
            .await
            .map_err(|e| crate::error::Error::ApiError(format!("Server error: {}", e)))?;

        Ok(())
    }

    /// Shutdown the server.
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ApiServerConfig::default();
        assert_eq!(config.bind_address, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert!(config.enable_cors);
    }

    #[test]
    fn test_app_state_creation() {
        let state = AppState::new();
        assert!(state.start_time.elapsed().as_secs() < 1);
    }

    #[test]
    fn test_server_creation() {
        let config = ApiServerConfig::default();
        let server = ApiServer::new(config);

        // Server should have a valid cancel token
        let token = server.cancel_token();
        assert!(!token.is_cancelled());
    }
}
