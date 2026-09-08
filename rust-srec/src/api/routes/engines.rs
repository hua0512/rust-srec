//! Engine configuration routes.
//!
//! Handles CRUD operations for download engine configurations.

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
    routing::get,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::models::{
    EngineConfigurationDbModel, EngineType, FfmpegEngineConfig, MesioEngineConfig,
    StreamlinkEngineConfig,
};
use crate::downloader::engine::{DownloadEngine, FfmpegEngine, MesioEngine, StreamlinkEngine};

#[derive(Clone)]
pub struct EngineRouteState {
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
}

impl FromRef<AppState> for EngineRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
        }
    }
}

/// Create the engines router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_engines).post(create_engine))
        .route(
            "/{id}",
            get(get_engine).patch(update_engine).delete(delete_engine),
        )
        .route("/{id}/test", get(test_engine))
}

/// Request model for creating a new engine configuration.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateEngineRequest {
    pub name: String,
    pub engine_type: EngineType,
    pub config: serde_json::Value,
}

/// Request model for updating an engine configuration.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateEngineRequest {
    pub name: Option<String>,
    pub engine_type: Option<EngineType>,
    pub config: Option<serde_json::Value>,
}

/// Response model for testing an engine.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EngineTestResponse {
    pub available: bool,
    pub version: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/engines",
    tag = "engines",
    responses(
        (status = 200, description = "List of engine configurations", body = Vec<crate::database::models::EngineConfigurationDbModel>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_engines(
    State(state): State<EngineRouteState>,
) -> ApiResult<Json<Vec<EngineConfigurationDbModel>>> {
    let config_service = &state.config_service;

    let engines = config_service
        .list_engine_configs()
        .await
        .map_err(ApiError::from)?;

    Ok(Json(engines))
}

#[utoipa::path(
    get,
    path = "/api/engines/{id}",
    tag = "engines",
    params(("id" = String, Path, description = "Engine config ID")),
    responses(
        (status = 200, description = "Engine configuration", body = crate::database::models::EngineConfigurationDbModel),
        (status = 404, description = "Engine not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_engine(
    State(state): State<EngineRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EngineConfigurationDbModel>> {
    let config_service = &state.config_service;

    let engine = config_service
        .get_engine_config(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(engine))
}

#[utoipa::path(
    post,
    path = "/api/engines",
    tag = "engines",
    request_body = CreateEngineRequest,
    responses(
        (status = 201, description = "Engine created", body = crate::database::models::EngineConfigurationDbModel)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_engine(
    State(state): State<EngineRouteState>,
    Json(request): Json<CreateEngineRequest>,
) -> ApiResult<(StatusCode, Json<EngineConfigurationDbModel>)> {
    let config_service = &state.config_service;

    let config_str = serde_json::to_string(&request.config)
        .map_err(|e| ApiError::bad_request(format!("Invalid config JSON: {}", e)))?;

    let engine = EngineConfigurationDbModel::new(request.name, request.engine_type, config_str);

    config_service
        .create_engine_config(&engine)
        .await
        .map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(engine)))
}

#[utoipa::path(
    patch,
    path = "/api/engines/{id}",
    tag = "engines",
    params(("id" = String, Path, description = "Engine config ID")),
    request_body = UpdateEngineRequest,
    responses(
        (status = 200, description = "Engine updated", body = crate::database::models::EngineConfigurationDbModel),
        (status = 404, description = "Engine not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_engine(
    State(state): State<EngineRouteState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateEngineRequest>,
) -> ApiResult<Json<EngineConfigurationDbModel>> {
    let config_service = &state.config_service;

    // Get current config to apply partial updates
    let mut engine = config_service
        .get_engine_config(&id)
        .await
        .map_err(ApiError::from)?;

    // Apply updates
    if let Some(name) = request.name {
        engine.name = name;
    }
    if let Some(engine_type) = request.engine_type {
        engine.engine_type = engine_type.as_str().to_string();
    }
    if let Some(config) = request.config {
        let config_str = serde_json::to_string(&config)
            .map_err(|e| ApiError::bad_request(format!("Invalid config JSON: {}", e)))?;
        engine.config = config_str;
    }

    config_service
        .update_engine_config(&engine)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(engine))
}

#[utoipa::path(
    delete,
    path = "/api/engines/{id}",
    tag = "engines",
    params(("id" = String, Path, description = "Engine config ID")),
    responses(
        (status = 200, description = "Engine deleted"),
        (status = 404, description = "Engine not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_engine(
    State(state): State<EngineRouteState>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    let config_service = &state.config_service;

    // Check if exists first
    config_service
        .get_engine_config(&id)
        .await
        .map_err(ApiError::from)?;

    config_service
        .delete_engine_config(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/engines/{id}/test",
    tag = "engines",
    params(("id" = String, Path, description = "Engine config ID")),
    responses(
        (status = 200, description = "Engine test result", body = EngineTestResponse),
        (status = 404, description = "Engine not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn test_engine(
    State(state): State<EngineRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EngineTestResponse>> {
    let config_service = &state.config_service;

    let config = config_service
        .get_engine_config(&id)
        .await
        .map_err(ApiError::from)?;

    let engine_type = EngineType::parse(&config.engine_type)
        .ok_or_else(|| ApiError::internal("Stored engine type is invalid"))?;

    let engine: Box<dyn DownloadEngine> = match engine_type {
        EngineType::Ffmpeg => {
            let engine_config: FfmpegEngineConfig =
                serde_json::from_str(&config.config).map_err(ApiError::from)?;
            Box::new(FfmpegEngine::with_config_async(engine_config).await)
        }
        EngineType::Streamlink => {
            let engine_config: StreamlinkEngineConfig =
                serde_json::from_str(&config.config).map_err(ApiError::from)?;
            Box::new(StreamlinkEngine::with_config_async(engine_config).await)
        }
        EngineType::Mesio => {
            let engine_config: MesioEngineConfig =
                serde_json::from_str(&config.config).map_err(ApiError::from)?;
            Box::new(MesioEngine::with_config(engine_config))
        }
    };

    Ok(Json(EngineTestResponse {
        available: engine.is_available(),
        version: engine.version(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigService;
    use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};
    use axum::http::StatusCode;
    use std::sync::Arc;

    #[tokio::test]
    async fn engine_routes_distinguish_typed_not_found_from_database_error_text() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE engine_configuration (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let state = EngineRouteState {
            config_service: Arc::new(ConfigService::new(
                Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
                Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone())),
            )),
        };
        for status in [StatusCode::NOT_FOUND, StatusCode::INTERNAL_SERVER_ERROR] {
            let errors = [
                get_engine(State(state.clone()), Path("missing".to_string()))
                    .await
                    .unwrap_err(),
                update_engine(
                    State(state.clone()),
                    Path("missing".to_string()),
                    Json(UpdateEngineRequest {
                        name: None,
                        engine_type: None,
                        config: None,
                    }),
                )
                .await
                .unwrap_err(),
                delete_engine(State(state.clone()), Path("missing".to_string()))
                    .await
                    .unwrap_err(),
                test_engine(State(state.clone()), Path("missing".to_string()))
                    .await
                    .unwrap_err(),
            ];
            for error in errors {
                assert_eq!(error.status, status);
                if status == StatusCode::INTERNAL_SERVER_ERROR {
                    assert_eq!(error.message, "Database error occurred");
                }
            }
            if status == StatusCode::NOT_FOUND {
                sqlx::query("DROP TABLE engine_configuration")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("CREATE VIEW engine_configuration AS SELECT * FROM \"not found\"")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        pool.close().await;
    }
}
