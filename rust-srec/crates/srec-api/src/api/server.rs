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
use tower_http::cors::Any;
use tower_http::trace::TraceLayer;
use tracing::Span;

use crate::api::cors::{self, CorsPolicy, OriginGuard};
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
    /// Load API server config from environment variables; unset variables keep their defaults.
    ///
    /// Supported env vars:
    /// - `API_BIND_ADDRESS` (e.g. "0.0.0.0")
    /// - `API_PORT` (e.g. "8080")
    ///
    /// # Errors
    ///
    /// Returns a configuration error when `API_PORT` is set to something other than a port number.
    pub fn from_env() -> crate::Result<Self> {
        let mut config = Self::default();

        if let Ok(bind_address) = std::env::var("API_BIND_ADDRESS")
            && !bind_address.trim().is_empty()
        {
            config.bind_address = bind_address;
        }

        if let Some(port) = crate::utils::env::parse("API_PORT")? {
            config.port = port;
        }

        Ok(config)
    }
}

use std::sync::Arc;

use crate::api::auth_service::{AuthPrincipal, AuthService};
use crate::config::ConfigService;
use crate::credentials::CredentialProviderRegistry;
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

/// A one-shot archive capability retains the issuing identity, never its raw credential.
#[derive(Clone)]
pub struct LoggingArchiveGrant {
    pub(crate) expires_at: jiff::Timestamp,
    pub(crate) principal: Option<AuthPrincipal>,
}

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
    pub pipeline_manager: Arc<PipelineManager>,
    /// Download manager
    pub download_manager: Arc<DownloadManager>,
    /// Read-pool query facade for UI/MCP filtered listing, search, and summary endpoints.
    pub queries: crate::api::queries::ApiQueries,
    /// Session repository for session and output queries
    pub session_repository: Arc<dyn SessionRepository>,
    /// Session-event repository for the session-detail timeline.
    pub session_event_repository: Arc<dyn SessionEventRepository>,
    /// Per-poll check-history repository for streamer details.
    pub streamer_check_history_repository: Arc<dyn StreamerCheckHistoryRepository>,
    /// Live broadcaster for committed check-history rows.
    pub check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    /// Live broadcaster for upload status events (started/progress/terminal).
    pub upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster,
    /// Durable per-file upload results for the uploads endpoints.
    pub upload_record_repository: Arc<dyn crate::database::repositories::UploadRecordRepository>,
    /// Filter repository for streamer filters
    pub filter_repository: Arc<dyn FilterRepository>,
    /// Health checker for real health status
    pub health_checker: Arc<HealthChecker>,
    /// Streamer repository for querying streamer details
    pub streamer_repository: Arc<dyn StreamerRepository>,
    /// Pipeline preset repository for pipeline presets (workflow sequences)
    pub pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
    /// Notification repository for channel/subscription management
    pub notification_repository: Arc<dyn NotificationRepository>,
    /// Notification service for testing and reloading
    pub notification_service: Arc<NotificationService>,
    /// Logging configuration for dynamic log level changes
    pub logging_config: Arc<crate::logging::LoggingConfig>,
    /// Single-use download tokens for log archives.
    pub logging_download_tokens: Arc<DashMap<String, LoggingArchiveGrant>>,
    /// Shared capacity and streaming archive generation for log downloads.
    pub logging_archives: Arc<crate::api::routes::logging::LogArchiveService>,
    /// Credential managers for account validation, refresh and capability checks.
    pub credential_service: Arc<CredentialProviderRegistry>,
    pub platform_admission: Arc<crate::credentials::PlatformAdmission>,
    pub credential_profiles:
        Arc<crate::database::repositories::credential_profiles::CredentialProfileRepository>,
    pub credential_login_sessions: Arc<crate::credentials::login_sessions::CredentialLoginSessions>,
    pub credential_execution: Arc<crate::credentials::CredentialExecutionService>,
    /// Saved proxies and their effects on resolved routes.
    pub proxies: Arc<crate::proxies::ProxyService>,
    /// Streamers whose latest credential acquisition found no usable account.
    pub credential_blocks: Arc<crate::credentials::CredentialBlocks>,
    /// Bigo's live-notification listener, for its status.
    pub bigo_notifications: Arc<crate::monitor::BigoNotifications>,
    pub playback_contexts: Arc<crate::api::playback_context::PlaybackContextService>,
    /// Shared stateless extractor runner (pooled HTTP clients and proxy/tunnel setup).
    pub extractor_runner: Arc<crate::recorder::PlatformExtractorRunner>,
    /// Rooms recordings are collecting, which live danmu sockets follow.
    pub danmu_taps: Arc<crate::danmu::DanmuTaps>,
    /// Transactional configuration export/import port implemented at the application layer.
    pub config_backup: Arc<dyn crate::api::backup::ConfigBackupPort>,
    /// Recording control-plane registry. Routes reach it for streamer deletion,
    /// which stands down the streamer actor, download, danmu, and pipeline
    /// owners before the `streamers` row can be removed.
    pub actor_registry: Arc<crate::recorder::ActorRegistry>,
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
    /// Cancellation token shared with [`ApiServer`] so long-lived streaming
    /// responses terminate cleanly when graceful shutdown begins.
    pub(crate) cancel_token: CancellationToken,
    /// Origins [`Self::cors_policy`] allows while `auth_service` is `None`,
    /// resolved once from `API_CORS_ORIGINS`.
    unauthenticated_cors_origins: Arc<[axum::http::HeaderValue]>,
}

impl AppState {
    /// Create a fully wired application state, installing the API layer's
    /// Protobuf WebSocket encoders onto the shared broadcasters if they were
    /// created without one.
    pub fn new(services: ApiServices) -> Self {
        services
            .check_history_broadcaster
            .set_encoder(routes::downloads::check_history_ws_encoder());
        services.upload_status_broadcaster.set_encoder(
            routes::downloads::upload_status_ws_encoder(services.streamer_manager.clone()),
        );
        Self {
            start_time: Instant::now(),
            auth_service: None,
            web_push_service: None,
            services: Arc::new(services),
            cancel_token: CancellationToken::new(),
            unauthenticated_cors_origins: cors::allowed_origins_from_env(),
        }
    }

    /// Cross-origin policy for this instance.
    ///
    /// Without `auth_service` every route is reachable with no credential, so
    /// only the explicit `API_CORS_ORIGINS` allowlist may reach it from a
    /// browser; with `auth_service` set, `AuthLayer` gates the routes and a
    /// foreign origin gains nothing from a permissive policy.
    pub fn cors_policy(&self) -> CorsPolicy {
        cors::policy_for(
            self.auth_service.is_some(),
            &self.unauthenticated_cors_origins,
        )
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
        let cancel_token = state.cancel_token.clone();
        Self {
            config,
            state,
            cancel_token,
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

        let policy = self.state.cors_policy();
        cors::log_policy(&policy);

        // `CorsPolicy::Exact` only governs response headers; without
        // `origin_guard` a foreign page could still reach handlers through
        // requests browsers do not preflight. Installed under the CorsLayer
        // below so preflights, which that layer answers itself, skip it.
        if matches!(policy, CorsPolicy::Exact(_)) {
            router = router.layer(axum::middleware::from_fn_with_state(
                OriginGuard::new(policy.clone(), &self.config.bind_address),
                cors::origin_guard,
            ));
        }

        // Add CORS if enabled
        if self.config.enable_cors {
            let cors = policy
                .layer()
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
                    let path = req.uri().path();
                    if path.starts_with("/api/health") {
                        return Span::none();
                    }
                    if let Some(sanitized_uri) = sanitized_span_uri(req.uri()) {
                        return tracing::span!(
                            target: "tower_http::trace::make_span",
                            tracing::Level::INFO,
                            "request",
                            method = %req.method(),
                            uri = %sanitized_uri,
                            version = ?req.version(),
                        );
                    }
                    let mut make_span =
                        tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO);
                    use tower_http::trace::MakeSpan;
                    make_span.make_span(req)
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
        // `into_make_service_with_connect_info` is what puts
        // `ConnectInfo<SocketAddr>` in request extensions, where
        // `api::rate_limit::ClientIp` reads the peer address that keys the
        // login failure counters.
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            cancel_token.cancelled().await;
            tracing::info!("API server shutting down...");
        })
        .await
        .map_err(|e| crate::error::Error::ApiError(format!("Server error: {}", e)))?;

        Ok(())
    }
}

/// Route prefixes that carry bearer tokens or upstream authentication headers
/// in the query string (`stream_proxy`, `media`, `downloads`, `logging`, and
/// `parse/danmu` WebSocket upgrades). Span recording strips the entire query
/// string on these routes so secrets never reach logs.
const TOKEN_QUERY_ROUTES: [&str; 5] = [
    "/api/stream-proxy",
    "/api/media",
    "/api/downloads",
    "/api/logging",
    "/api/parse/danmu",
];

/// Compute a sanitized span `uri` when `uri` hits a token-bearing route or
/// carries `token=` / `headers=` query parameters; returns `None` when the raw
/// URI is safe for `DefaultMakeSpan`.
pub(crate) fn sanitized_span_uri(uri: &axum::http::Uri) -> Option<String> {
    let path = uri.path();
    if TOKEN_QUERY_ROUTES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return Some(path.to_string());
    }
    redact_sensitive_query_uri(uri)
}

/// Redact `token` and `headers` query values as defense-in-depth before
/// building request spans on routes outside `TOKEN_QUERY_ROUTES`.
pub(crate) fn redact_sensitive_query_uri(uri: &axum::http::Uri) -> Option<String> {
    let query = uri.query()?;
    let has_sensitive = url::form_urlencoded::parse(query.as_bytes())
        .any(|(key, _)| key.eq_ignore_ascii_case("token") || key.eq_ignore_ascii_case("headers"));
    if !has_sensitive {
        return None;
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key.eq_ignore_ascii_case("token") || key.eq_ignore_ascii_case("headers") {
            ser.append_pair(&key, "[REDACTED]");
        } else {
            ser.append_pair(&key, &value);
        }
    }
    let redacted = ser.finish();
    Some(format!("{}?{}", uri.path(), redacted))
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::sanitized_span_uri;

    #[test]
    fn sanitizes_danmu_ws_and_redacts_sensitive_query_params() {
        let danmu_uri: Uri = "/api/parse/danmu/ws?token=super-secret-jwt"
            .parse()
            .unwrap();
        assert_eq!(
            sanitized_span_uri(&danmu_uri).as_deref(),
            Some("/api/parse/danmu/ws")
        );

        let proxy_uri: Uri = "/api/stream-proxy/stream?url=https%3A%2F%2Fexample.com&token=secret"
            .parse()
            .unwrap();
        assert_eq!(
            sanitized_span_uri(&proxy_uri).as_deref(),
            Some("/api/stream-proxy/stream")
        );

        let stray_token_uri: Uri =
            "/api/sessions?page=2&token=jwt-value&Headers=Authorization%3A+Bearer+abc"
                .parse()
                .unwrap();
        let redacted = sanitized_span_uri(&stray_token_uri).expect("redacted URI");
        assert_eq!(
            redacted,
            "/api/sessions?page=2&token=%5BREDACTED%5D&Headers=%5BREDACTED%5D"
        );
        assert!(!redacted.contains("jwt-value"));
        assert!(!redacted.contains("Bearer"));

        let benign_uri: Uri = "/api/sessions?page=2&limit=20".parse().unwrap();
        assert_eq!(sanitized_span_uri(&benign_uri), None);
    }
}
