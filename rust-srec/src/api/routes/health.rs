//! Health check routes.

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};

use crate::api::error::ApiResult;
use crate::api::models::{ComponentHealth, HealthResponse};
use crate::api::server::AppState;

/// Create the health router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(health_check))
        .route("/ready", get(readiness_check))
        .route("/live", get(liveness_check))
}

/// Health check endpoint.
async fn health_check(State(state): State<AppState>) -> ApiResult<Json<HealthResponse>> {
    let uptime = state.start_time.elapsed().as_secs();

    // Use HealthChecker if available, otherwise return fallback response
    if let Some(health_checker) = &state.health_checker {
        let system_health = health_checker.check_all().await;

        let components: Vec<ComponentHealth> = system_health
            .components
            .into_iter()
            .map(|(name, health)| ComponentHealth {
                name,
                status: health.status.to_string(),
                message: health.message,
            })
            .collect();

        let response = HealthResponse {
            status: system_health.status.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: uptime,
            components,
        };

        Ok(Json(response))
    } else {
        // Fallback for testing without full service setup
        Ok(Json(HealthResponse {
            status: "healthy".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: uptime,
            components: vec![],
        }))
    }
}

/// Readiness check - is the service ready to accept traffic?
/// Returns HTTP 200 if healthy/degraded, HTTP 503 if unhealthy/unknown.
async fn readiness_check(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(health_checker) = &state.health_checker {
        let is_ready = health_checker.check_ready().await;
        if is_ready {
            (StatusCode::OK, "ready")
        } else {
            (StatusCode::SERVICE_UNAVAILABLE, "not ready")
        }
    } else {
        // Fallback for testing without full service setup
        (StatusCode::OK, "ready")
    }
}

/// Liveness check - is the service alive?
/// Returns HTTP 200 with status and uptime if the service is responsive.
async fn liveness_check(State(state): State<AppState>) -> impl IntoResponse {
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
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("database"));
    }
}
