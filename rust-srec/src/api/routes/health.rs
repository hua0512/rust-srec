//! Health check routes.

use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
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
    let Some(auth_service) = &state.auth_service else {
        return Ok(());
    };

    let token = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| {
            crate::api::error::ApiError::unauthorized("Missing or invalid Authorization header")
        })?;

    auth_service
        .authorize_access_token(token, false)
        .await
        .map_err(crate::api::error::ApiError::from)?;

    Ok(())
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
pub async fn liveness_check(State(state): State<HealthRouteState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "alive",
            "uptime_secs": uptime
        })),
    )
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
    let processing_jobs = state.pipeline_manager.get_stats().await?.processing;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_idle_test_state(
        pipeline_manager: crate::pipeline::PipelineManager,
    ) -> HealthRouteState {
        HealthRouteState {
            start_time: std::time::Instant::now(),
            auth_service: None,
            health_checker: std::sync::Arc::new(crate::metrics::HealthChecker::new()),
            download_manager: std::sync::Arc::new(crate::downloader::DownloadManager::new()),
            pipeline_manager: std::sync::Arc::new(pipeline_manager),
        }
    }

    async fn read_json(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        serde_json::from_slice(&body).expect("body should be valid JSON")
    }

    /// Probe standing in for `DiskSpaceProbe`, which lives in
    /// `services::container::health` and is not constructible from here.
    struct StaticDiskProbe;

    #[async_trait::async_trait]
    impl crate::metrics::HealthProbe for StaticDiskProbe {
        fn name(&self) -> std::borrow::Cow<'_, str> {
            std::borrow::Cow::Borrowed("disk:/rec")
        }

        fn cadence(&self) -> std::time::Duration {
            std::time::Duration::from_secs(30)
        }

        async fn probe(
            &self,
            _metrics: crate::metrics::SystemMetricsSnapshot,
        ) -> crate::metrics::ComponentHealth {
            crate::metrics::ComponentHealth::healthy("disk:/rec").with_disk(
                crate::metrics::DiskUsage::new(
                    "/rec",
                    "/",
                    40 * 1024 * 1024 * 1024,
                    100 * 1024 * 1024 * 1024,
                ),
            )
        }
    }

    #[tokio::test]
    async fn health_check_serializes_disk_capacity() {
        let health_checker = std::sync::Arc::new(crate::metrics::HealthChecker::new());
        health_checker.register_probe(std::sync::Arc::new(StaticDiskProbe));
        let cancel = tokio_util::sync::CancellationToken::new();
        let handle = health_checker.start(cancel.child_token());
        // `start` runs a first-fill refresh before its ticker, so one yield
        // is enough for the snapshot to hold the probe's value.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let state = HealthRouteState {
            health_checker,
            ..build_idle_test_state(crate::pipeline::PipelineManager::new())
        };
        let response = health_check(State(state), HeaderMap::new())
            .await
            .expect("health_check should succeed")
            .into_response();
        let json = read_json(response).await;

        let disk = json["components"]
            .as_array()
            .expect("components should be an array")
            .iter()
            .find(|c| c["name"] == "disk:/rec")
            .expect("the disk component should be present");
        assert_eq!(disk["status"], "healthy");
        assert_eq!(disk["disk"]["mount_point"], "/");
        assert_eq!(disk["disk"]["available_bytes"], 40u64 * 1024 * 1024 * 1024);
        assert_eq!(disk["disk"]["used_bytes"], 60u64 * 1024 * 1024 * 1024);

        // Non-disk components must not grow a null `disk` key.
        let database = json["components"]
            .as_array()
            .and_then(|components| components.iter().find(|c| c["name"] == "database"));
        assert!(
            database.is_none_or(|c| c.get("disk").is_none()),
            "only disk components should carry capacity"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_idle_check_ok_when_nothing_active() {
        let state = build_idle_test_state(crate::pipeline::PipelineManager::new());

        let response = idle_check(State(state))
            .await
            .expect("idle_check should succeed")
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let json = read_json(response).await;
        assert_eq!(json["idle"], serde_json::Value::Bool(true));
        assert_eq!(json["active_recordings"], 0);
        assert_eq!(json["pending_recordings"], 0);
        assert_eq!(json["processing_jobs"], 0);
    }

    #[tokio::test]
    async fn test_idle_check_busy_when_job_processing() {
        let pipeline_manager = crate::pipeline::PipelineManager::new();
        let mut job = crate::pipeline::Job::new(
            "remux",
            vec!["input.mp4".to_string()],
            vec!["output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        job.status = crate::database::models::JobStatus::Processing;
        pipeline_manager
            .enqueue(job)
            .await
            .expect("enqueue should succeed");

        let state = build_idle_test_state(pipeline_manager);
        let response = idle_check(State(state))
            .await
            .expect("idle_check should succeed")
            .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let json = read_json(response).await;
        assert_eq!(json["idle"], serde_json::Value::Bool(false));
        assert_eq!(json["processing_jobs"], 1);
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "healthy".to_string(),
            version: "0.1.0".to_string(),
            uptime_secs: 3600,
            components: vec![ComponentHealth {
                name: "database".to_string(),
                status: "healthy".to_string(),
                message: None,
                last_check: None,
                check_duration_ms: None,
                disk: None,
            }],
            cpu_usage: 10.5,
            memory_usage: 45.2,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("database"));
    }
}
