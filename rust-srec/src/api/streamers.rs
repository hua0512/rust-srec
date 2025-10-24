use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    api::AppState,
    domain::{streamer::Streamer, types::StreamerState},
};

pub fn streamers_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_streamers))
        .route("/", post(create_streamer))
        .route("/statuses", get(get_streamer_statuses))
        .route("/:id", get(get_streamer))
        .route("/:id", put(update_streamer))
        .route("/:id", delete(delete_streamer))
        .route("/:id/check", post(trigger_check))
        .route("/:id/clear_error", post(clear_error_state))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub message: String,
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
pub struct CreateStreamer {
    pub name: String,
    pub url: String,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateStreamer {
    pub name: Option<String>,
    pub url: Option<String>,
    pub platform_config_id: Option<String>,
    pub template_config_id: Option<Option<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StreamerStatus {
    pub id: String,
    pub name: String,
    pub state: StreamerState,
}

#[utoipa::path(
    get,
    path = "/api/streamers",
    responses(
        (status = 200, description = "List all streamers successfully", body = [Streamer]),
        (status = 500, description = "Failed to list streamers", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn list_streamers(
    State(state): State<AppState>,
) -> Result<Json<Vec<Streamer>>, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.streamers().find_all().await {
        Ok(streamers) => Ok(Json(streamers)),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to list streamers: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/streamers",
    request_body = CreateStreamer,
    responses(
        (status = 201, description = "Streamer created successfully", body = Streamer),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 500, description = "Failed to create streamer", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn create_streamer(
    State(state): State<AppState>,
    Json(payload): Json<CreateStreamer>,
) -> Result<(StatusCode, Json<Streamer>), (StatusCode, Json<ErrorResponse>)> {
    let platform_config = match state
        .db_service
        .platform_configs()
        .find_by_id(&payload.platform_config_id)
        .await
    {
        Ok(Some(config)) => config,
        Ok(None) => {
            let error_response = ErrorResponse {
                message: format!(
                    "Platform config with id {} not found",
                    payload.platform_config_id
                ),
            };
            return Err((StatusCode::BAD_REQUEST, Json(error_response)));
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to get platform config: {}", e),
            };
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)));
        }
    };

    let streamer = Streamer {
        id: Uuid::new_v4().to_string(),
        name: payload.name,
        url: payload.url.into(),
        platform: platform_config.platform_name,
        state: StreamerState::NotLive,
        consecutive_error_count: 0,
        disabled_until: None,
        config: Default::default(),
        filters: vec![],
        live_sessions: vec![],
        platform_config_id: payload.platform_config_id,
        template_config_id: payload.template_config_id,
    };

    match state.db_service.streamers().create(&streamer).await {
        Ok(_) => Ok((StatusCode::CREATED, Json(streamer))),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to create streamer: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/streamers/{id}",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 200, description = "Streamer found", body = Streamer),
        (status = 404, description = "Streamer not found", body = ErrorResponse),
        (status = 500, description = "Failed to get streamer", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn get_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Streamer>, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.streamers().find_by_id(&id).await {
        Ok(Some(streamer)) => Ok(Json(streamer)),
        Ok(None) => {
            let error_response = ErrorResponse {
                message: format!("Streamer with id {} not found", id),
            };
            Err((StatusCode::NOT_FOUND, Json(error_response)))
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to get streamer: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/streamers/{id}",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    request_body = UpdateStreamer,
    responses(
        (status = 200, description = "Streamer updated successfully", body = Streamer),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 404, description = "Streamer not found", body = ErrorResponse),
        (status = 500, description = "Failed to update streamer", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn update_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateStreamer>,
) -> Result<Json<Streamer>, (StatusCode, Json<ErrorResponse>)> {
    let mut streamer = match state.db_service.streamers().find_by_id(&id).await {
        Ok(Some(streamer)) => streamer,
        Ok(None) => {
            let error_response = ErrorResponse {
                message: format!("Streamer with id {} not found", id),
            };
            return Err((StatusCode::NOT_FOUND, Json(error_response)));
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to get streamer: {}", e),
            };
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)));
        }
    };

    if let Some(name) = payload.name {
        streamer.name = name;
    }
    if let Some(url) = payload.url {
        streamer.url = url.into();
    }
    if let Some(platform_config_id) = payload.platform_config_id {
        streamer.platform_config_id = platform_config_id;
    }
    if let Some(template_config_id) = payload.template_config_id {
        streamer.template_config_id = template_config_id;
    }

    match state.db_service.streamers().update(&streamer).await {
        Ok(_) => Ok(Json(streamer)),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to update streamer: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/streamers/{id}",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 204, description = "Streamer deleted successfully"),
        (status = 404, description = "Streamer not found", body = ErrorResponse),
        (status = 500, description = "Failed to delete streamer", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn delete_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.streamers().delete(&id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to delete streamer: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/streamers/statuses",
    responses(
        (status = 200, description = "List all streamer statuses successfully", body = [StreamerStatus]),
        (status = 500, description = "Failed to list streamer statuses", body = ErrorResponse)
    )
)]
async fn get_streamer_statuses(
    State(state): State<AppState>,
) -> Result<Json<Vec<StreamerStatus>>, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.streamers().find_all().await {
        Ok(streamers) => {
            let statuses = streamers
                .into_iter()
                .map(|s| StreamerStatus {
                    id: s.id,
                    name: s.name,
                    state: s.state,
                })
                .collect();
            Ok(Json(statuses))
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to list streamer statuses: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/streamers/{id}/check",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 202, description = "Streamer check triggered successfully"),
        (status = 501, description = "Not implemented")
    )
)]
async fn trigger_check(
    Path(_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            message: "Not implemented".to_string(),
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/streamers/{id}/clear_error",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 200, description = "Streamer error state cleared successfully"),
        (status = 501, description = "Not implemented")
    )
)]
async fn clear_error_state(
    Path(_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            message: "Not implemented".to_string(),
        }),
    ))
}