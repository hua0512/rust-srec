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
    jwt_service: Option<std::sync::Arc<crate::api::jwt::JwtService>>,
    health_checker: std::sync::Arc<crate::metrics::HealthChecker>,
}

impl FromRef<AppState> for HealthRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            start_time: state.start_time,
            jwt_service: state.jwt_service.clone(),
            health_checker: state.health_checker.clone(),
        }
    }
}

/// Create the health router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(health_check))
        .route("/ready", get(readiness_check))
        .route("/live", get(liveness_check))
}

fn validate_health_auth(
    headers: &HeaderMap,
    state: &HealthRouteState,
) -> Result<(), crate::api::error::ApiError> {
    let jwt_service = state.jwt_service.as_ref().ok_or_else(|| {
        crate::api::error::ApiError::unauthorized("Authentication not configured")
    })?;

    let token = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| {
            crate::api::error::ApiError::unauthorized("Missing or invalid Authorization header")
        })?;

    jwt_service
        .validate_token(token)
        .map_err(|_| crate::api::error::ApiError::unauthorized("Invalid or expired token"))?;

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
    validate_health_auth(&headers, &state)?;
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
    if let Err(err) = validate_health_auth(&headers, &state) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            }],
            cpu_usage: 10.5,
            memory_usage: 45.2,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("database"));
    }
}
