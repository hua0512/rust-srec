//! API server setup and configuration.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::header;
use axum::serve::ListenerExt;
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
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

use crate::api::auth_service::AuthService;
use crate::config::ConfigService;
use crate::credentials::CredentialRefreshService;
use crate::database::repositories::{
    config::SqlxConfigRepository,
    filter::FilterRepository,
    preset::PipelinePresetRepository,
    session::SessionRepository,
    session_event::SessionEventRepository,
    streamer::{SqlxStreamerRepository, StreamerRepository},
    streamer_check_history::StreamerCheckHistoryRepository,
};
use crate::downloader::DownloadManager;
use crate::metrics::HealthChecker;
use crate::notification::web_push::WebPushService;
use crate::pipeline::PipelineManager;
use crate::streamer::StreamerManager;

/// Services required by the API router.
///
/// Keeping these dependencies required makes an incompletely wired API
/// impossible to construct. Optional product capabilities live on
/// [`AppState`] instead.
pub struct ApiServices {
    /// Configuration service
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    /// Streamer manager
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    /// Pipeline manager
    pub pipeline_manager: Arc<PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>>,
    /// Download manager
    pub download_manager: Arc<DownloadManager>,
    /// Session repository for session and output queries
    pub session_repository: Arc<dyn SessionRepository>,
    /// Session-event repository for the session-detail timeline.
    pub session_event_repository: Arc<dyn SessionEventRepository>,
    /// Per-poll check-history repository for streamer details.
    pub streamer_check_history_repository: Arc<dyn StreamerCheckHistoryRepository>,
    /// Live broadcaster for committed check-history rows.
    pub check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    /// Filter repository for streamer filters
    pub filter_repository: Arc<dyn FilterRepository>,
    /// Health checker for real health status
    pub health_checker: Arc<HealthChecker>,
    /// Streamer repository for querying streamer details
    pub streamer_repository: Arc<dyn StreamerRepository>,
    /// Pipeline preset repository for pipeline presets (workflow sequences)
    pub pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
    /// Job preset repository for job presets (reusable processor configs)
    pub job_preset_repository: Arc<dyn crate::database::repositories::JobPresetRepository>,
    /// Notification repository for channel/subscription management
    pub notification_repository: Arc<dyn NotificationRepository>,
    /// Notification service for testing and reloading
    pub notification_service: Arc<NotificationService>,
    /// Logging configuration for dynamic log level changes
    pub logging_config: Arc<crate::logging::LoggingConfig>,
    /// Single-use download tokens for log archives.
    pub logging_download_tokens: Arc<DashMap<String, chrono::DateTime<chrono::Utc>>>,
    /// Credential refresh service for API-triggered refresh and cookie resolution.
    pub credential_service: Arc<CredentialRefreshService<SqlxConfigRepository>>,
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Server start time for uptime calculation
    pub start_time: Instant,
    /// Authentication and token service. `None` means authentication is explicitly disabled.
    pub auth_service: Option<Arc<AuthService>>,
    /// Browser push capability. `None` means VAPID is not configured.
    pub web_push_service: Option<Arc<WebPushService>>,
    /// Services that every API instance requires.
    pub services: Arc<ApiServices>,
}

impl AppState {
    /// Create a fully wired application state.
    pub fn new(services: ApiServices) -> Self {
        Self {
            start_time: Instant::now(),
            auth_service: None,
            web_push_service: None,
            services: Arc::new(services),
        }
    }

    /// Set the auth service.
    pub fn with_auth_service(mut self, auth_service: Arc<AuthService>) -> Self {
        self.auth_service = Some(auth_service);
        self
    }

    /// Set the web push service.
    pub fn with_web_push_service(mut self, service: Arc<WebPushService>) -> Self {
        self.web_push_service = Some(service);
        self
    }
}

impl std::ops::Deref for AppState {
    type Target = ApiServices;

    fn deref(&self) -> &Self::Target {
        &self.services
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
    pub fn new(config: ApiServerConfig, state: AppState) -> Self {
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

        router = router.layer(DefaultBodyLimit::max(self.config.body_limit));

        // Add CORS if enabled
        if self.config.enable_cors {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .expose_headers([
                    header::ACCEPT_RANGES,
                    header::CONTENT_LENGTH,
                    header::CONTENT_RANGE,
                ]);
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
                        tower_http::trace::DefaultOnRequest::new().level(tracing::Level::DEBUG);
                    use tower_http::trace::OnRequest;
                    on_request.on_request(req, span);
                })
                .on_response(
                    |res: &axum::http::Response<_>, latency: Duration, span: &Span| {
                        if span.is_disabled() {
                            return;
                        }
                        let level = if latency >= Duration::from_millis(200) {
                            tracing::Level::INFO
                        } else {
                            tracing::Level::DEBUG
                        };
                        let on_response = tower_http::trace::DefaultOnResponse::new().level(level);
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

    /// Bind a TCP listener for the configured address and return the resolved local address.
    ///
    /// This is useful when binding to port `0` (ephemeral port), where the actual port is only
    /// known after binding.
    pub async fn bind(&self) -> Result<(TcpListener, SocketAddr)> {
        let bind_address: IpAddr = self.config.bind_address.trim().parse().map_err(|error| {
            crate::error::Error::config(format!(
                "Invalid API_BIND_ADDRESS '{}': {error}",
                self.config.bind_address
            ))
        })?;
        let addr = SocketAddr::new(bind_address, self.config.port);

        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok((listener, local_addr))
    }

    /// Start the server using an already-bound listener.
    pub async fn run_with_listener(&self, listener: TcpListener) -> Result<()> {
        let router = self.build_router();

        let listener = listener.tap_io(|tcp_stream| {
            if let Err(err) = tcp_stream.set_nodelay(true) {
                tracing::trace!(error = %err, "failed to set TCP_NODELAY on incoming connection");
            }
        });

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
}
