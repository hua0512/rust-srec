use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::server::AppState;
use crate::database::models::notification::{ChannelType, NotificationChannelDbModel};
use crate::notification::events::{NotificationEventTypeInfo, notification_event_types};
use crate::notification::service::NotificationChannelInstance;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/event-types", get(list_event_types))
        // Channels CRUD
        .route("/channels", get(list_channels).post(create_channel))
        .route(
            "/channels/{id}",
            get(get_channel).put(update_channel).delete(delete_channel),
        )
        // Subscriptions
        .route(
            "/channels/{id}/subscriptions",
            get(get_subscriptions).put(update_subscriptions),
        )
        // Testing
        .route("/channels/{id}/test", post(test_channel))
        // List active instances (debug/status)
        .route("/instances", get(list_instances))
}

// DTOs

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub channel_type: ChannelType,
    pub settings: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub name: String,
    pub settings: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSubscriptionsRequest {
    pub events: Vec<String>,
}

// Handlers

async fn list_event_types() -> Json<Vec<NotificationEventTypeInfo>> {
    Json(notification_event_types().to_vec())
}

async fn list_instances(
    State(state): State<AppState>,
) -> Result<Json<Vec<NotificationChannelInstance>>, ApiError> {
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;
    Ok(Json(service.list_channel_instances()))
}

async fn list_channels(
    State(state): State<AppState>,
) -> Result<Json<Vec<NotificationChannelDbModel>>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;

    let channels = repo
        .list_channels()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(channels))
}

async fn get_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NotificationChannelDbModel>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;

    let channel = repo
        .get_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(channel))
}

async fn create_channel(
    State(state): State<AppState>,
    Json(req): Json<CreateChannelRequest>,
) -> Result<Json<NotificationChannelDbModel>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;

    // Create model
    let settings_str = serde_json::to_string(&req.settings)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings JSON: {}", e)))?;

    let channel = NotificationChannelDbModel::new(req.name, req.channel_type, settings_str);

    // Save to DB
    repo.create_channel(&channel)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Reload service to pick up new channel immediately
    if let Err(e) = service.reload_from_db().await {
        tracing::error!("Failed to reload notification service: {}", e);
    }

    Ok(Json(channel))
}

async fn update_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateChannelRequest>,
) -> Result<Json<NotificationChannelDbModel>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;

    let mut channel = repo
        .get_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    channel.name = req.name;
    channel.settings = serde_json::to_string(&req.settings)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings JSON: {}", e)))?;

    repo.update_channel(&channel)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Reload service
    if let Err(e) = service.reload_from_db().await {
        tracing::error!("Failed to reload notification service: {}", e);
    }

    Ok(Json(channel))
}

async fn delete_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;

    repo.delete_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Reload service
    if let Err(e) = service.reload_from_db().await {
        tracing::error!("Failed to reload notification service: {}", e);
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn get_subscriptions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;

    let subs = repo
        .get_subscriptions_for_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(subs))
}

async fn update_subscriptions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSubscriptionsRequest>,
) -> Result<Json<Vec<String>>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;

    // Verify channel exists
    repo.get_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Get existing
    let existing = repo
        .get_subscriptions_for_channel(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Determine diff
    let new_set: std::collections::HashSet<_> = req.events.iter().cloned().collect();
    let old_set: std::collections::HashSet<_> = existing.into_iter().collect();

    // Add new
    for event in new_set.difference(&old_set) {
        repo.subscribe(&id, event)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    // Remove old
    for event in old_set.difference(&new_set) {
        repo.unsubscribe(&id, event)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    // Reload service
    if let Err(e) = service.reload_from_db().await {
        tracing::error!("Failed to reload notification service: {}", e);
    }

    Ok(Json(req.events))
}

async fn test_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;

    // The service requires the "key" which for DB channels is the ID
    service
        .test_channel_instance(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(StatusCode::OK)
}
