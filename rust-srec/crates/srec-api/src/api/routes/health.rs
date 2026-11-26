//! Health check routes.

use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};

use crate::api::error::ApiResult;
use crate::api::models::{ComponentHealth, HealthResponse};
use crate::api::server::AppState;

#[derive(Clone)]
pub struct HealthRouteState {
    start_time: std::time::Instant,
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    health_checker: std::sync::Arc<crate::metrics::HealthChecker>,
    download_manager: std::sync::Arc<crate::downloader::DownloadManager>,
    pipeline_manager: std::sync::Arc<crate::pipeline::PipelineManager>,
}

impl FromRef<AppState> for HealthRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            start_time: state.start_time,
            auth_service: state.auth_service.clone(),
            health_checker: state.health_checker.clone(),
            download_manager: state.download_manager.clone(),
            pipeline_manager: state.pipeline_manager.clone(),
        }
    }
}

/// Active optional backend capabilities advertised through health endpoints.
pub(crate) fn active_capabilities() -> Vec<String> {
    Vec::new()
}

/// Create the health router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(health_check))
        .route("/ready", get(readiness_check))
        .route("/live", get(liveness_check))
        .route("/idle", get(idle_check))
}

async fn validate_health_auth(
    headers: &HeaderMap,
    state: &HealthRouteState,
) -> Result<(), crate::api::error::ApiError> {
    crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers,
        None,
        crate::api::auth_request::AccessPolicy::Read,
    )
    .await
    .map(|_| ())
}

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "health",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Full health check response", body = HealthResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn health_check(
    State(state): State<HealthRouteState>,
    headers: HeaderMap,
) -> ApiResult<Json<HealthResponse>> {
    validate_health_auth(&headers, &state).await?;
    let uptime = state.start_time.elapsed().as_secs();

    let system_health = state.health_checker.current();
    let components: Vec<ComponentHealth> = system_health
        .components
        .iter()
        .map(|(name, health)| ComponentHealth {
            name: name.clone(),
            status: health.status.to_string(),
            message: health.message.clone(),
            last_check: health.last_check.clone(),
            check_duration_ms: health.check_duration_ms,
            disk: health.disk.clone(),
        })
        .collect();

    Ok(Json(HealthResponse {
        status: system_health.status.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: uptime,
        components,
        cpu_usage: system_health.cpu_usage,
        memory_usage: system_health.memory_usage,
        capabilities: active_capabilities(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/health/ready",
    tag = "health",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Service is ready"),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Service not ready")
    )
)]
pub async fn readiness_check(
    State(state): State<HealthRouteState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(err) = validate_health_auth(&headers, &state).await {
        return err.into_response();
    }

    if state.health_checker.check_ready() {
        (StatusCode::OK, "ready").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
    }
}

#[utoipa::path(
    get,
    path = "/api/health/live",
    tag = "health",
    responses(
        (status = 200, description = "Service is alive", body = crate::api::openapi::LivenessResponse)
    )
)]
pub async fn liveness_check(
    State(state): State<HealthRouteState>,
) -> Json<crate::api::openapi::LivenessResponse> {
    Json(crate::api::openapi::LivenessResponse {
        status: "alive".to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
    })
}

/// Idle-state response for the container auto-update gate.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct IdleResponse {
    /// True when no recording is active or queued and no pipeline job is processing.
    pub idle: bool,
    /// Downloads currently writing to disk (`DownloadManager::active_count`).
    pub active_recordings: usize,
    /// Downloads queued for a slot (`DownloadManager::pending_count`).
    pub pending_recordings: usize,
    /// Pipeline jobs in `Processing` status: uploads, remux, danmaku conversion, etc.
    pub processing_jobs: u64,
}

/// Restart-safety gate for container auto-updaters (e.g. a Watchtower
/// pre-update hook): 200 means a restart interrupts nothing, 503 means a
/// recording or pipeline job would be cut short. Pipeline jobs in `Pending`
/// do not block: `JobQueue::recover_jobs` re-runs them after a restart.
///
/// Unauthenticated like `liveness_check` — in-container hook scripts have
/// no JWT, and the body only discloses coarse counts.
#[utoipa::path(
    get,
    path = "/api/health/idle",
    tag = "health",
    responses(
        (status = 200, description = "System is idle; safe to restart for an update", body = IdleResponse),
        (status = 503, description = "Recording or pipeline processing in progress", body = IdleResponse)
    )
)]
pub async fn idle_check(State(state): State<HealthRouteState>) -> ApiResult<impl IntoResponse> {
    let active_recordings = state.download_manager.active_count();
    let pending_recordings = state.download_manager.pending_count();
    let processing_jobs = state.pipeline_manager.count_processing_jobs().await?;

    let idle = active_recordings == 0 && pending_recordings == 0 && processing_jobs == 0;
    let status = if idle {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    Ok((
        status,
        Json(IdleResponse {
            idle,
            active_recordings,
            pending_recordings,
            processing_jobs,
        }),
    ))
}
