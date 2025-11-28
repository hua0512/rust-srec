//! Health check routes.

use axum::{extract::State, routing::get, Json, Router};
use std::time::Instant;

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
    
    let mut components = Vec::new();
    
    // Check database
    components.push(ComponentHealth {
        name: "database".to_string(),
        status: "healthy".to_string(),
        message: None,
    });
    
    // Check scheduler
    components.push(ComponentHealth {
        name: "scheduler".to_string(),
        status: "healthy".to_string(),
        message: None,
    });
    
    // Check download manager
    components.push(ComponentHealth {
        name: "download_manager".to_string(),
        status: "healthy".to_string(),
        message: None,
    });
    
    // Check pipeline manager
    components.push(ComponentHealth {
        name: "pipeline_manager".to_string(),
        status: "healthy".to_string(),
        message: None,
    });

    let response = HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: uptime,
        components,
    };

    Ok(Json(response))
}

/// Readiness check - is the service ready to accept traffic?
async fn readiness_check() -> &'static str {
    "ready"
}

/// Liveness check - is the service alive?
async fn liveness_check() -> &'static str {
    "alive"
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
            components: vec![
                ComponentHealth {
                    name: "database".to_string(),
                    status: "healthy".to_string(),
                    message: None,
                },
            ],
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("database"));
    }
}
