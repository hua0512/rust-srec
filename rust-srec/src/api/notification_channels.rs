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
    api::{error::ApiError, AppState},
    domain::{
        notification_channel::NotificationChannel,
        types::{NotificationChannelSettings, NotificationChannelType},
    },
};

pub fn notification_channels_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_notification_channels))
        .route("/", post(create_notification_channel))
        .route("/:id", get(get_notification_channel))
        .route("/:id", put(update_notification_channel))
        .route("/:id", delete(delete_notification_channel))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateNotificationChannel {
    pub name: String,
    pub channel_type: NotificationChannelType,
    pub settings: NotificationChannelSettings,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateNotificationChannel {
    pub name: Option<String>,
    pub channel_type: Option<NotificationChannelType>,
    pub settings: Option<NotificationChannelSettings>,
}

#[utoipa::path(
    get,
    path = "/api/notification_channels",
    responses(
        (status = 200, description = "List all notification channels successfully", body = [NotificationChannel]),
        (status = 500, description = "Failed to list notification channels", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn list_notification_channels(
    State(state): State<AppState>,
) -> Result<Json<Vec<NotificationChannel>>, ApiError> {
    let channels = state
        .db_service
        .notification_channels()
        .find_all()
        .await?;
    Ok(Json(channels))
}

#[utoipa::path(
    post,
    path = "/api/notification_channels",
    request_body = CreateNotificationChannel,
    responses(
        (status = 201, description = "Notification channel created successfully", body = NotificationChannel),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 500, description = "Failed to create notification channel", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn create_notification_channel(
    State(state): State<AppState>,
    Json(payload): Json<CreateNotificationChannel>,
) -> Result<(StatusCode, Json<NotificationChannel>), ApiError> {
    let channel = NotificationChannel {
        id: Uuid::new_v4().to_string(),
        name: payload.name,
        channel_type: payload.channel_type,
        settings: payload.settings,
    };

    state
        .db_service
        .notification_channels()
        .create(&channel)
        .await?;

    Ok((StatusCode::CREATED, Json(channel)))
}

#[utoipa::path(
    get,
    path = "/api/notification_channels/{id}",
    params(
        ("id" = String, Path, description = "Notification channel id")
    ),
    responses(
        (status = 200, description = "Notification channel found", body = NotificationChannel),
        (status = 404, description = "Notification channel not found", body = ErrorResponse),
        (status = 500, description = "Failed to get notification channel", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn get_notification_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NotificationChannel>, ApiError> {
    let channel = state
        .db_service
        .notification_channels()
        .find_by_id(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Notification channel with id {} not found", id)))?;
    Ok(Json(channel))
}

#[utoipa::path(
    put,
    path = "/api/notification_channels/{id}",
    params(
        ("id" = String, Path, description = "Notification channel id")
    ),
    request_body = UpdateNotificationChannel,
    responses(
        (status = 200, description = "Notification channel updated successfully", body = NotificationChannel),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 404, description = "Notification channel not found", body = ErrorResponse),
        (status = 500, description = "Failed to update notification channel", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn update_notification_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateNotificationChannel>,
) -> Result<Json<NotificationChannel>, ApiError> {
    let mut channel = state
        .db_service
        .notification_channels()
        .find_by_id(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Notification channel with id {} not found", id)))?;

    if let Some(name) = payload.name {
        channel.name = name;
    }
    if let Some(channel_type) = payload.channel_type {
        channel.channel_type = channel_type;
    }
    if let Some(settings) = payload.settings {
        channel.settings = settings;
    }

    state
        .db_service
        .notification_channels()
        .update(&channel)
        .await?;

    Ok(Json(channel))
}

#[utoipa::path(
    delete,
    path = "/api/notification_channels/{id}",
    params(
        ("id" = String, Path, description = "Notification channel id")
    ),
    responses(
        (status = 204, description = "Notification channel deleted successfully"),
        (status = 404, description = "Notification channel not found", body = ErrorResponse),
        (status = 500, description = "Failed to delete notification channel", body = ErrorResponse)
    )
)]
#[axum::debug_handler]
async fn delete_notification_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .db_service
        .notification_channels()
        .delete(&id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}