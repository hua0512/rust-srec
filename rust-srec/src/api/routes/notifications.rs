use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use base64::Engine as _;
use url::Url;

use crate::api::error::ApiError;
use crate::api::jwt::Claims;
use crate::api::server::AppState;
use crate::database::models::notification::{
    ChannelType, NotificationChannelDbModel, NotificationEventLogDbModel,
};
use crate::notification::NotificationPriority;
use crate::notification::events::{NotificationEventTypeInfo, notification_event_types};
use crate::notification::service::NotificationChannelInstance;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/event-types", get(list_event_types))
        .route("/events", get(list_events))
        // Web Push (VAPID) for browser notifications
        .route("/web-push/public-key", get(get_web_push_public_key))
        .route("/web-push/subscriptions", get(list_web_push_subscriptions))
        .route("/web-push/subscribe", post(subscribe_web_push))
        .route("/web-push/unsubscribe", post(unsubscribe_web_push))
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

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateChannelRequest {
    pub name: String,
    pub channel_type: ChannelType,
    pub settings: serde_json::Value,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateChannelRequest {
    pub name: String,
    pub settings: serde_json::Value,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateSubscriptionsRequest {
    pub events: Vec<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ListEventsQuery {
    /// Max number of events to return (default: 200, max: 1000).
    pub limit: Option<i32>,
    /// Row offset for pagination (default: 0).
    pub offset: Option<i32>,
    /// Filter by event type (e.g., "stream_online", "credential_refresh_failed").
    pub event_type: Option<String>,
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Search by streamer name (case-insensitive).
    pub search: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct WebPushPublicKeyResponse {
    pub public_key: String,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct WebPushSubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct WebPushSubscriptionJson {
    pub endpoint: String,
    pub keys: WebPushSubscriptionKeys,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct WebPushSubscriptionResponse {
    pub id: String,
    pub endpoint: String,
    pub min_priority: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct SubscribeWebPushRequest {
    pub subscription: WebPushSubscriptionJson,
    /// Minimum priority to send ("low"|"normal"|"high"|"critical"), default: "critical".
    pub min_priority: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UnsubscribeWebPushRequest {
    pub endpoint: String,
}

// Handlers

#[utoipa::path(
    get,
    path = "/api/notifications/event-types",
    tag = "notifications",
    responses(
        (status = 200, description = "List of event types", body = Vec<crate::notification::events::NotificationEventTypeInfo>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_event_types() -> Json<Vec<NotificationEventTypeInfo>> {
    Json(notification_event_types().to_vec())
}

#[utoipa::path(
    get,
    path = "/api/notifications/web-push/public-key",
    tag = "notifications",
    responses(
        (status = 200, description = "VAPID public key (base64url)", body = WebPushPublicKeyResponse),
        (status = 503, description = "Web push not configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_web_push_public_key(
    State(state): State<AppState>,
) -> Result<Json<WebPushPublicKeyResponse>, ApiError> {
    let service = state
        .web_push_service
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;

    Ok(Json(WebPushPublicKeyResponse {
        public_key: service.vapid_public_key().to_string(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/notifications/web-push/subscriptions",
    tag = "notifications",
    responses(
        (status = 200, description = "List of web push subscriptions for the current user", body = Vec<WebPushSubscriptionResponse>),
        (status = 503, description = "Web push not configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_web_push_subscriptions(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<Vec<WebPushSubscriptionResponse>>, ApiError> {
    let service = state
        .web_push_service
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;

    let rows = service
        .list_subscriptions_for_user(&claims.sub)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(
        rows.into_iter()
            .map(|r| WebPushSubscriptionResponse {
                id: r.id,
                endpoint: r.endpoint,
                min_priority: r.min_priority,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/notifications/web-push/subscribe",
    tag = "notifications",
    request_body = SubscribeWebPushRequest,
    responses(
        (status = 200, description = "Web push subscription stored", body = WebPushSubscriptionResponse),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Web push not configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn subscribe_web_push(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<SubscribeWebPushRequest>,
) -> Result<Json<WebPushSubscriptionResponse>, ApiError> {
    let service = state
        .web_push_service
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;

    let endpoint = req.subscription.endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("subscription.endpoint is required"));
    }
    let endpoint_url = Url::parse(endpoint)
        .map_err(|e| ApiError::bad_request(format!("Invalid endpoint URL: {}", e)))?;
    let host = endpoint_url
        .host_str()
        .ok_or_else(|| ApiError::bad_request("subscription.endpoint missing host"))?;
    let is_localhost = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if endpoint_url.scheme() != "https" && !(endpoint_url.scheme() == "http" && is_localhost) {
        return Err(ApiError::bad_request(
            "subscription.endpoint must use https scheme (or http for localhost)",
        ));
    }

    let p256dh_raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(req.subscription.keys.p256dh.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("Invalid subscription.keys.p256dh: {}", e)))?;
    if p256dh_raw.len() != 65 {
        return Err(ApiError::bad_request(
            "subscription.keys.p256dh must decode to 65 bytes",
        ));
    }

    let auth_raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(req.subscription.keys.auth.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("Invalid subscription.keys.auth: {}", e)))?;
    if auth_raw.len() != 16 {
        return Err(ApiError::bad_request(
            "subscription.keys.auth must decode to 16 bytes",
        ));
    }

    let min_priority = match req.min_priority.as_deref() {
        None => NotificationPriority::Critical,
        Some(value) => parse_priority(value).ok_or_else(|| {
            ApiError::bad_request(format!(
                "Invalid min_priority '{}'. Expected one of: low, normal, high, critical",
                value
            ))
        })?,
    };

    let saved = service
        .upsert_subscription(
            &claims.sub,
            endpoint,
            &req.subscription.keys.p256dh,
            &req.subscription.keys.auth,
            min_priority,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(WebPushSubscriptionResponse {
        id: saved.id,
        endpoint: saved.endpoint,
        min_priority: saved.min_priority,
        created_at: saved.created_at,
        updated_at: saved.updated_at,
    }))
}

#[utoipa::path(
    post,
    path = "/api/notifications/web-push/unsubscribe",
    tag = "notifications",
    request_body = UnsubscribeWebPushRequest,
    responses(
        (status = 200, description = "Web push subscription removed"),
        (status = 503, description = "Web push not configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn unsubscribe_web_push(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<UnsubscribeWebPushRequest>,
) -> Result<StatusCode, ApiError> {
    let service = state
        .web_push_service
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;

    let endpoint = req.endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("endpoint is required"));
    }

    service
        .unsubscribe(&claims.sub, endpoint)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/notifications/events",
    tag = "notifications",
    params(
        ("limit" = Option<i32>, Query, description = "Max events to return (default: 200, max: 1000)"),
        ("offset" = Option<i32>, Query, description = "Row offset for pagination (default: 0)"),
        ("event_type" = Option<String>, Query, description = "Filter by event type"),
        ("streamer_id" = Option<String>, Query, description = "Filter by streamer id"),
        ("search" = Option<String>, Query, description = "Search by streamer name (case-insensitive)")
    ),
    responses(
        (status = 200, description = "List of events", body = Vec<crate::database::models::notification::NotificationEventLogDbModel>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_events(
    State(state): State<AppState>,
    Query(q): Query<ListEventsQuery>,
) -> Result<Json<Vec<NotificationEventLogDbModel>>, ApiError> {
    let repo = state
        .notification_repository
        .ok_or_else(|| ApiError::service_unavailable("Notification repository not available"))?;

    let limit = q.limit.unwrap_or(200);
    let offset = q.offset.unwrap_or(0);
    let entries = repo
        .list_event_logs(
            q.event_type.as_deref(),
            q.streamer_id.as_deref(),
            q.search.as_deref(),
            offset,
            limit,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(entries))
}

fn parse_priority(input: &str) -> Option<NotificationPriority> {
    match input.trim().to_ascii_lowercase().as_str() {
        "low" => Some(NotificationPriority::Low),
        "normal" => Some(NotificationPriority::Normal),
        "high" => Some(NotificationPriority::High),
        "critical" => Some(NotificationPriority::Critical),
        _ => None,
    }
}

#[utoipa::path(
    get,
    path = "/api/notifications/instances",
    tag = "notifications",
    responses(
        (status = 200, description = "List of active channel instances", body = Vec<crate::notification::service::NotificationChannelInstance>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_instances(
    State(state): State<AppState>,
) -> Result<Json<Vec<NotificationChannelInstance>>, ApiError> {
    let service = state
        .notification_service
        .ok_or_else(|| ApiError::service_unavailable("Notification service not available"))?;
    Ok(Json(service.list_channel_instances()))
}

#[utoipa::path(
    get,
    path = "/api/notifications/channels",
    tag = "notifications",
    responses(
        (status = 200, description = "List of channels", body = Vec<crate::database::models::notification::NotificationChannelDbModel>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_channels(
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

#[utoipa::path(
    get,
    path = "/api/notifications/channels/{id}",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    responses(
        (status = 200, description = "Channel details", body = crate::database::models::notification::NotificationChannelDbModel),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_channel(
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

#[utoipa::path(
    post,
    path = "/api/notifications/channels",
    tag = "notifications",
    request_body = CreateChannelRequest,
    responses(
        (status = 201, description = "Channel created", body = crate::database::models::notification::NotificationChannelDbModel)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_channel(
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

#[utoipa::path(
    put,
    path = "/api/notifications/channels/{id}",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    request_body = UpdateChannelRequest,
    responses(
        (status = 200, description = "Channel updated", body = crate::database::models::notification::NotificationChannelDbModel),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_channel(
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

#[utoipa::path(
    delete,
    path = "/api/notifications/channels/{id}",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    responses(
        (status = 204, description = "Channel deleted"),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_channel(
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

#[utoipa::path(
    get,
    path = "/api/notifications/channels/{id}/subscriptions",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    responses(
        (status = 200, description = "List of subscribed event types", body = Vec<String>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_subscriptions(
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

#[utoipa::path(
    put,
    path = "/api/notifications/channels/{id}/subscriptions",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    request_body = UpdateSubscriptionsRequest,
    responses(
        (status = 200, description = "Subscriptions updated", body = Vec<String>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_subscriptions(
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

#[utoipa::path(
    post,
    path = "/api/notifications/channels/{id}/test",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    responses(
        (status = 200, description = "Test notification sent"),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn test_channel(
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
