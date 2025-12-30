//! API server setup and configuration.

use axum::Router;
use axum::extract::Request;
use std::net::SocketAddr;
use std::time::Duration;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::Span;

use crate::api::routes;
use crate::database::repositories::NotificationRepository;
use crate::error::Result;
use crate::notification::NotificationService;

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

impl ApiServerConfig {
    /// Load API server config from environment variables, falling back to defaults.
    ///
    /// Supported env vars:
    /// - `API_BIND_ADDRESS` (e.g. "0.0.0.0")
    /// - `API_PORT` (e.g. "8080")
    pub fn from_env_or_default() -> Self {
        let mut config = Self::default();

        if let Ok(bind_address) = std::env::var("API_BIND_ADDRESS")
            && !bind_address.trim().is_empty()
        {
            config.bind_address = bind_address;
        }

        if let Ok(port) = std::env::var("API_PORT")
            && let Ok(parsed) = port.parse::<u16>()
        {
            config.port = parsed;
        }

        config
    }
}

use std::sync::Arc;

use crate::api::auth_service::{AuthConfig, AuthService};
use crate::api::jwt::JwtService;
use crate::config::ConfigService;
use crate::credentials::CredentialRefreshService;
use crate::danmu::DanmuService;
use crate::database::repositories::{
    config::SqlxConfigRepository,
    filter::FilterRepository,
    preset::PipelinePresetRepository,
    session::SessionRepository,
    streamer::{SqlxStreamerRepository, StreamerRepository},
};
use crate::downloader::DownloadManager;
use crate::metrics::HealthChecker;
use crate::pipeline::PipelineManager;
use crate::streamer::StreamerManager;
use platforms_parser::extractor::create_client_builder;

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
    pub pipeline_manager:
        Option<Arc<PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>>>,
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
    /// Streamer repository for querying streamer details
    pub streamer_repository: Option<Arc<dyn StreamerRepository>>,
    /// Pipeline preset repository for pipeline presets (workflow sequences)
    pub pipeline_preset_repository: Option<Arc<dyn PipelinePresetRepository>>,
    /// Job preset repository for job presets (reusable processor configs)
    pub job_preset_repository: Option<Arc<dyn crate::database::repositories::JobPresetRepository>>,
    /// Notification repository for channel/subscription management
    pub notification_repository: Option<Arc<dyn NotificationRepository>>,
    /// Notification service for testing and reloading
    pub notification_service: Option<Arc<NotificationService>>,
    /// Logging configuration for dynamic log level changes
    pub logging_config: Option<Arc<crate::logging::LoggingConfig>>,
    /// Shared HTTP client for parsing/resolving URLs
    pub http_client: Option<reqwest::Client>,
    /// Optional credential refresh service for API-triggered refresh and cookie resolution.
    pub credential_service: Option<Arc<CredentialRefreshService<SqlxConfigRepository>>>,
}

impl AppState {
    pub(crate) fn build_http_client() -> reqwest::Client {
        match create_client_builder(None).build() {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Failed to create HTTP client via platforms-parser; falling back to reqwest defaults"
                );
                reqwest::Client::new()
            }
        }
    }

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
            streamer_repository: None,
            pipeline_preset_repository: None,
            job_preset_repository: None,
            notification_repository: None,
            notification_service: None,
            logging_config: None,
            http_client: Some(Self::build_http_client()),
            credential_service: None,
        }
    }

    /// Create a new application state with JWT service from environment variables.
    pub fn with_jwt_from_env() -> Self {
        let auth_config = AuthConfig::from_env();
        let jwt_service =
            JwtService::from_env(auth_config.access_token_expiration_secs).map(Arc::new);
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
            streamer_repository: None,
            pipeline_preset_repository: None,
            job_preset_repository: None,
            notification_repository: None,
            notification_service: None,
            logging_config: None,
            http_client: Some(Self::build_http_client()),
            credential_service: None,
        }
    }

    /// Create application state with all services.
    pub fn with_services(
        jwt_service: Option<Arc<JwtService>>,
        config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
        pipeline_manager: Arc<PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>>,
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
            streamer_repository: None,
            pipeline_preset_repository: None,
            job_preset_repository: None,
            notification_repository: None,
            notification_service: None,
            logging_config: None,
            http_client: Some(Self::build_http_client()),
            credential_service: None,
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

    /// Set the streamer repository.
    pub fn with_streamer_repository(
        mut self,
        streamer_repository: Arc<dyn StreamerRepository>,
    ) -> Self {
        self.streamer_repository = Some(streamer_repository);
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

    /// Set the credential refresh service.
    pub fn with_credential_service(
        mut self,
        credential_service: Arc<CredentialRefreshService<SqlxConfigRepository>>,
    ) -> Self {
        self.credential_service = Some(credential_service);
        self
    }

    /// Set the pipeline preset repository.
    pub fn with_pipeline_preset_repository(
        mut self,
        repo: Arc<dyn PipelinePresetRepository>,
    ) -> Self {
        self.pipeline_preset_repository = Some(repo);
        self
    }

    /// Set the job preset repository.
    pub fn with_job_preset_repository(
        mut self,
        repo: Arc<dyn crate::database::repositories::JobPresetRepository>,
    ) -> Self {
        self.job_preset_repository = Some(repo);
        self
    }

    /// Set the notification repository.
    pub fn with_notification_repository(mut self, repo: Arc<dyn NotificationRepository>) -> Self {
        self.notification_repository = Some(repo);
        self
    }

    /// Set the notification service.
    pub fn with_notification_service(mut self, service: Arc<NotificationService>) -> Self {
        self.notification_service = Some(service);
        self
    }

    /// Set the logging configuration.
    pub fn with_logging_config(mut self, config: Arc<crate::logging::LoggingConfig>) -> Self {
        self.logging_config = Some(config);
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
        router = router.layer(
            TraceLayer::new_for_http()
                .make_span_with(|req: &Request| {
                    if req.uri().path().starts_with("/api/health") {
                        Span::none()
                    } else {
                        let mut make_span =
                            tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO);
                        use tower_http::trace::MakeSpan;
                        make_span.make_span(req)
                    }
                })
                .on_request(|req: &Request, span: &Span| {
                    if span.is_disabled() || req.uri().path().starts_with("/api/health") {
                        return;
                    }
                    let mut on_request =
                        tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO);
                    use tower_http::trace::OnRequest;
                    on_request.on_request(req, span);
                })
                .on_response(
                    |res: &axum::http::Response<_>, latency: Duration, span: &Span| {
                        if span.is_disabled() {
                            return;
                        }
                        let on_response =
                            tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO);
                        use tower_http::trace::OnResponse;
                        on_response.on_response(res, latency, span);
                    },
                )
                .on_failure(
                    |class: tower_http::classify::ServerErrorsFailureClass,
                     latency: Duration,
                     span: &Span| {
                        if span.is_disabled() {
                            return;
                        }
                        let mut on_failure =
                            tower_http::trace::DefaultOnFailure::new().level(tracing::Level::ERROR);
                        use tower_http::trace::OnFailure;
                        on_failure.on_failure(class, latency, span);
                    },
                ),
        );
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
        assert_eq!(config.port, 12555);
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
