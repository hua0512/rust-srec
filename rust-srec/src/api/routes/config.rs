//! Configuration routes.

use axum::{
    extract::{Path, State},
    routing::{get, patch},
    Json, Router,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    GlobalConfigResponse, PlatformConfigResponse, UpdateGlobalConfigRequest,
    UpdatePlatformConfigRequest,
};
use crate::api::server::AppState;

/// Create the config router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/global", get(get_global_config))
        .route("/global", patch(update_global_config))
        .route("/platforms", get(list_platform_configs))
        .route("/platforms/:id", get(get_platform_config))
        .route("/platforms/:id", patch(update_platform_config))
}

/// Get global configuration.
async fn get_global_config(
    State(state): State<AppState>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    // TODO: Implement actual retrieval logic using ConfigService
    let response = GlobalConfigResponse {
        output_folder: "./downloads".to_string(),
        output_filename_template: "{streamer_name}-{title}-{timestamp}".to_string(),
        output_file_format: "flv".to_string(),
        max_concurrent_downloads: 6,
        max_concurrent_uploads: 3,
        max_concurrent_cpu_jobs: 0,
        max_concurrent_io_jobs: 8,
        streamer_check_delay_ms: 60000,
        offline_check_delay_ms: 20000,
        offline_check_count: 3,
        default_download_engine: "default_mesio".to_string(),
        record_danmu: false,
    };

    Ok(Json(response))
}

/// Update global configuration.
async fn update_global_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateGlobalConfigRequest>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    // TODO: Implement actual update logic using ConfigService
    // For now, return the same defaults
    let response = GlobalConfigResponse {
        output_folder: request.output_folder.unwrap_or_else(|| "./downloads".to_string()),
        output_filename_template: request.output_filename_template.unwrap_or_else(|| "{streamer_name}-{title}-{timestamp}".to_string()),
        output_file_format: request.output_file_format.unwrap_or_else(|| "flv".to_string()),
        max_concurrent_downloads: request.max_concurrent_downloads.unwrap_or(6),
        max_concurrent_uploads: request.max_concurrent_uploads.unwrap_or(3),
        max_concurrent_cpu_jobs: request.max_concurrent_cpu_jobs.unwrap_or(0),
        max_concurrent_io_jobs: request.max_concurrent_io_jobs.unwrap_or(8),
        streamer_check_delay_ms: request.streamer_check_delay_ms.unwrap_or(60000),
        offline_check_delay_ms: request.offline_check_delay_ms.unwrap_or(20000),
        offline_check_count: request.offline_check_count.unwrap_or(3),
        default_download_engine: request.default_download_engine.unwrap_or_else(|| "default_mesio".to_string()),
        record_danmu: request.record_danmu.unwrap_or(false),
    };

    Ok(Json(response))
}

/// List all platform configurations.
async fn list_platform_configs(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<PlatformConfigResponse>>> {
    // TODO: Implement actual listing logic using ConfigService
    Ok(Json(Vec::new()))
}

/// Get a specific platform configuration.
async fn get_platform_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    // TODO: Implement actual retrieval logic using ConfigService
    Err(ApiError::not_found(format!("Platform config with id '{}' not found", id)))
}

/// Update a platform configuration.
async fn update_platform_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePlatformConfigRequest>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    // TODO: Implement actual update logic using ConfigService
    Err(ApiError::not_found(format!("Platform config with id '{}' not found", id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_config_response_serialization() {
        let response = GlobalConfigResponse {
            output_folder: "./downloads".to_string(),
            output_filename_template: "{name}".to_string(),
            output_file_format: "flv".to_string(),
            max_concurrent_downloads: 6,
            max_concurrent_uploads: 3,
            max_concurrent_cpu_jobs: 0,
            max_concurrent_io_jobs: 8,
            streamer_check_delay_ms: 60000,
            offline_check_delay_ms: 20000,
            offline_check_count: 3,
            default_download_engine: "mesio".to_string(),
            record_danmu: false,
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("downloads"));
        assert!(json.contains("mesio"));
    }
}
