use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::StatusCode,
    routing::{get, post},
};
use base64::Engine as _;
use srec_core::CoreError;

use crate::api::auth_service::AuthPrincipal;
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::server::AppState;
use crate::database::models::notification::{
    ChannelType, NotificationChannelDbModel, NotificationEventLogDbModel,
};
use crate::notification::NotificationPriority;
use crate::notification::events::{
    NotificationEventTypeInfo, canonicalize_subscription_event_name, notification_event_types,
};
use crate::notification::service::NotificationChannelInstance;
use crate::proxies::ProxyRoute;

#[derive(Clone)]
pub struct NotificationRouteState {
    auth_enabled: bool,
    queries: crate::api::queries::ApiQueries,
    notification_repository:
        std::sync::Arc<dyn crate::database::repositories::NotificationRepository>,
    notification_service: std::sync::Arc<crate::notification::NotificationService>,
    web_push_service: Option<std::sync::Arc<crate::notification::web_push::WebPushService>>,
}

impl FromRef<AppState> for NotificationRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_enabled: state.auth_service.is_some(),
            queries: state.queries.clone(),
            notification_repository: state.notification_repository.clone(),
            notification_service: state.notification_service.clone(),
            web_push_service: state.web_push_service.clone(),
        }
    }
}

impl NotificationRouteState {
    async fn resolve_web_push_user_id(
        &self,
        identity: Option<axum::Extension<AuthPrincipal>>,
    ) -> Result<Option<String>, ApiError> {
        if self.auth_enabled {
            return super::request_principal(true, identity).map(Some);
        }
        self.notification_repository
            .default_web_push_user_id()
            .await
            .map_err(ApiError::from)
    }
}

/// Brings the running channels in line with a change that is already stored.
/// A failed reload is logged rather than failing the request: the client would
/// see an error for a change that took effect, and a retried create would add
/// a second channel.
async fn reload_channels(service: &crate::notification::NotificationService) {
    if let Err(error) = service.reload_from_db().await {
        tracing::error!(%error, "Failed to reload notification channels after a stored change");
    }
}

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
    #[schema(value_type = crate::api::schemas::ChannelType)]
    pub channel_type: ChannelType,
    pub settings: serde_json::Value,
    /// How deliveries connect; `inherit`, the default, follows the global
    /// route. Email channels always connect directly and accept only
    /// `inherit`.
    #[serde(default)]
    #[schema(value_type = crate::api::schemas::ProxyRoute)]
    pub proxy_route: ProxyRoute,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateChannelRequest {
    pub name: String,
    pub settings: serde_json::Value,
    /// Replaces the channel's route; omitting it keeps it.
    #[serde(default)]
    #[schema(value_type = Option<crate::api::schemas::ProxyRoute>)]
    pub proxy_route: Option<ProxyRoute>,
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
    /// Literal substring of streamer name or stored payload JSON; ASCII case-insensitive.
    pub search: Option<String>,
    /// Filter by minimum priority level (low, normal, high, critical).
    pub priority: Option<String>,
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
    pub min_priority: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct SubscribeWebPushRequest {
    pub subscription: WebPushSubscriptionJson,
    /// Minimum priority to send (integer 0-10 scale, or legacy string), default: 10 (critical).
    pub min_priority: Option<serde_json::Value>,
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
        (status = 200, description = "List of event types", body = Vec<crate::api::schemas::NotificationEventTypeInfo>)
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
    State(state): State<NotificationRouteState>,
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
    State(state): State<NotificationRouteState>,
    identity: Option<axum::Extension<AuthPrincipal>>,
) -> Result<Json<Vec<WebPushSubscriptionResponse>>, ApiError> {
    let service = state
        .web_push_service
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;
    let Some(user_id) = state.resolve_web_push_user_id(identity).await? else {
        return Ok(Json(Vec::new()));
    };

    let rows = service.list_subscriptions_for_user(&user_id).await?;

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
    State(state): State<NotificationRouteState>,
    identity: Option<axum::Extension<AuthPrincipal>>,
    ApiJson(req): ApiJson<SubscribeWebPushRequest>,
) -> Result<Json<WebPushSubscriptionResponse>, ApiError> {
    let service = state
        .web_push_service
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;
    let user_id = state
        .resolve_web_push_user_id(identity)
        .await?
        .ok_or_else(|| {
            ApiError::service_unavailable(
                "No local user account available for Web Push subscription",
            )
        })?;

    let endpoint = req.subscription.endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("subscription.endpoint is required"));
    }
    service
        .validate_endpoint(endpoint)
        .await
        .map_err(|error| match error {
            crate::notification::NotifyError::Core(CoreError::Validation(msg)) => {
                ApiError::bad_request(msg)
            }
            other => ApiError::from(other),
        })?;

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

    let min_priority = match &req.min_priority {
        None => NotificationPriority::Critical,
        Some(serde_json::Value::Number(n)) => {
            let val = n
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| {
                    ApiError::bad_request("min_priority must be an integer between 0 and 10")
                })?;
            NotificationPriority::from_int(val).unwrap_or(NotificationPriority::Critical)
        }
        Some(serde_json::Value::String(s)) => parse_priority(s).ok_or_else(|| {
            ApiError::bad_request(format!(
                "Invalid min_priority '{}'. Expected integer (0-10) or one of: low, normal, high, critical",
                s
            ))
        })?,
        _ => NotificationPriority::Critical,
    };

    let saved = service
        .upsert_subscription(
            &user_id,
            endpoint,
            &req.subscription.keys.p256dh,
            &req.subscription.keys.auth,
            min_priority,
        )
        .await?;

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
    State(state): State<NotificationRouteState>,
    identity: Option<axum::Extension<AuthPrincipal>>,
    ApiJson(req): ApiJson<UnsubscribeWebPushRequest>,
) -> Result<StatusCode, ApiError> {
    let service = state
        .web_push_service
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Web push is not configured"))?;
    let Some(user_id) = state.resolve_web_push_user_id(identity).await? else {
        return Ok(StatusCode::OK);
    };

    let endpoint = req.endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("endpoint is required"));
    }

    service.unsubscribe(&user_id, endpoint).await?;

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
        ("search" = Option<String>, Query, description = "Literal substring of streamer name or stored payload JSON; ASCII case-insensitive"),
        ("priority" = Option<String>, Query, description = "Filter by minimum priority level (low, normal, high, critical)")
    ),
    responses(
        (status = 200, description = "List of events", body = Vec<crate::api::schemas::NotificationEventLogDbModel>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_events(
    State(state): State<NotificationRouteState>,
    ApiQuery(q): ApiQuery<ListEventsQuery>,
) -> Result<Json<Vec<NotificationEventLogDbModel>>, ApiError> {
    let limit = q.limit.unwrap_or(200);
    let offset = q.offset.unwrap_or(0);
    let entries = state
        .queries
        .list_event_logs(
            q.event_type.as_deref(),
            q.streamer_id.as_deref(),
            q.search.as_deref(),
            q.priority.as_deref(),
            offset,
            limit,
        )
        .await?;

    Ok(Json(entries))
}

fn parse_priority(input: &str) -> Option<NotificationPriority> {
    // Try integer first (new format), then string (legacy).
    if let Ok(int_val) = input.trim().parse::<u8>() {
        return NotificationPriority::from_int(int_val);
    }
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
        (status = 200, description = "List of active channel instances", body = Vec<crate::api::schemas::NotificationChannelInstance>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_instances(
    State(state): State<NotificationRouteState>,
) -> Result<Json<Vec<NotificationChannelInstance>>, ApiError> {
    let service = &state.notification_service;
    Ok(Json(service.list_channel_instances()))
}

#[utoipa::path(
    get,
    path = "/api/notifications/channels",
    tag = "notifications",
    responses(
        (status = 200, description = "List of channels", body = Vec<crate::api::schemas::NotificationChannelDbModel>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_channels(
    State(state): State<NotificationRouteState>,
) -> Result<Json<Vec<NotificationChannelDbModel>>, ApiError> {
    let repo = &state.notification_repository;

    let channels = repo.list_channels().await?;
    Ok(Json(channels))
}

#[utoipa::path(
    get,
    path = "/api/notifications/channels/{id}",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    responses(
        (status = 200, description = "Channel details", body = crate::api::schemas::NotificationChannelDbModel),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_channel(
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<NotificationChannelDbModel>, ApiError> {
    let repo = &state.notification_repository;

    let channel = repo.get_channel(&id).await?;
    Ok(Json(channel))
}

#[utoipa::path(
    post,
    path = "/api/notifications/channels",
    tag = "notifications",
    request_body = CreateChannelRequest,
    responses(
        (status = 201, description = "Channel created", body = crate::api::schemas::NotificationChannelDbModel),
        (status = 422, description = "The route names a saved proxy that does not exist (`PROXY_NOT_FOUND`), or an email channel chooses a route other than inherit (`VALIDATION_ERROR`): email always connects directly", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_channel(
    State(state): State<NotificationRouteState>,
    ApiJson(req): ApiJson<CreateChannelRequest>,
) -> Result<(StatusCode, Json<NotificationChannelDbModel>), ApiError> {
    let repo = &state.notification_repository;
    let service = &state.notification_service;

    // Create model
    let settings_str = serde_json::to_string(&req.settings)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings JSON: {}", e)))?;

    let mut channel = NotificationChannelDbModel::new(req.name, req.channel_type, settings_str);
    channel.proxy_route = req.proxy_route;

    // Save to DB
    repo.create_channel(&channel).await?;

    reload_channels(service).await;

    Ok((StatusCode::CREATED, Json(channel)))
}

#[utoipa::path(
    put,
    path = "/api/notifications/channels/{id}",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    request_body = UpdateChannelRequest,
    responses(
        (status = 200, description = "Channel updated", body = crate::api::schemas::NotificationChannelDbModel),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "The route names a saved proxy that does not exist (`PROXY_NOT_FOUND`), or an email channel chooses a route other than inherit (`VALIDATION_ERROR`): email always connects directly", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_channel(
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateChannelRequest>,
) -> Result<Json<NotificationChannelDbModel>, ApiError> {
    let repo = &state.notification_repository;
    let service = &state.notification_service;

    let settings = serde_json::to_string(&req.settings)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings JSON: {}", e)))?;
    // An omitted route is left out of the write, so this edit cannot revert
    // a route changed since the client read the channel.
    let channel = repo
        .patch_channel(
            &id,
            crate::database::repositories::notification::ChannelPatch {
                name: req.name,
                settings,
                proxy_route: req.proxy_route,
            },
        )
        .await?;

    reload_channels(service).await;

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
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let repo = &state.notification_repository;
    let service = &state.notification_service;

    repo.delete_channel(&id).await?;

    reload_channels(service).await;

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
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let repo = &state.notification_repository;

    let subs = repo.get_subscriptions_for_channel(&id).await?;
    Ok(Json(subs))
}

#[utoipa::path(
    put,
    path = "/api/notifications/channels/{id}/subscriptions",
    tag = "notifications",
    params(("id" = String, Path, description = "Channel ID")),
    request_body = UpdateSubscriptionsRequest,
    responses(
        (status = 200, description = "Subscriptions updated", body = Vec<String>),
        (status = 400, description = "Unknown or retired notification event name", body = crate::api::error::ApiErrorResponse),
        (status = 404, description = "Channel not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_subscriptions(
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateSubscriptionsRequest>,
) -> Result<Json<Vec<String>>, ApiError> {
    let repo = &state.notification_repository;
    let service = &state.notification_service;

    let mut seen = std::collections::HashSet::with_capacity(req.events.len());
    let mut canonical_events = Vec::with_capacity(req.events.len());
    for event in &req.events {
        let canonical = canonicalize_subscription_event_name(event).ok_or_else(|| {
            ApiError::bad_request(format!("Unknown notification event type: '{event}'"))
        })?;
        if seen.insert(canonical) {
            canonical_events.push(canonical.to_string());
        }
    }

    repo.replace_subscriptions(&id, &canonical_events).await?;

    reload_channels(service).await;

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
    State(state): State<NotificationRouteState>,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let service = &state.notification_service;

    // The service requires the "key" which for DB channels is the ID
    service.test_channel_instance(&id).await?;

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::database::repositories::SqlxNotificationRepository;
    use crate::notification::NotificationService;
    use crate::notification::web_push::{WebPushConfig, WebPushService, generate_vapid_keypair};

    #[tokio::test]
    async fn web_push_routes_work_when_auth_disabled_and_reject_private_endpoints() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        let repo = Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone()));
        let notification_service = Arc::new(NotificationService::with_repository(
            Default::default(),
            repo.clone(),
        ));

        let keypair = generate_vapid_keypair();
        let pub_b64 = keypair.public_key_b64.clone();
        let auth_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 16]);
        let web_push_config = WebPushConfig::from_vapid(
            keypair.public_key_b64,
            keypair.private_key_b64,
            "mailto:test@localhost",
        )
        .unwrap();
        let web_push_service =
            Arc::new(WebPushService::new(pool.clone(), pool.clone(), web_push_config).unwrap());

        let state = NotificationRouteState {
            auth_enabled: false,
            queries: crate::api::queries::ApiQueries::new(pool.clone()),
            notification_repository: repo,
            notification_service,
            web_push_service: Some(web_push_service),
        };

        let app = Router::new()
            .route("/web-push/subscriptions", get(list_web_push_subscriptions))
            .route("/web-push/subscribe", post(subscribe_web_push))
            .route("/web-push/unsubscribe", post(unsubscribe_web_push))
            .with_state(state);

        // Rejects private metadata IP with 400 BAD_REQUEST.
        let ssrf_body = serde_json::json!({
            "subscription": {
                "endpoint": "https://169.254.169.254/latest/meta-data",
                "keys": { "p256dh": pub_b64, "auth": auth_b64 }
            }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/web-push/subscribe")
                    .header("content-type", "application/json")
                    .body(Body::from(ssrf_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Works without Extension<AuthPrincipal> when AUTH_DISABLED=true.
        let ok_body = serde_json::json!({
            "subscription": {
                "endpoint": "http://127.0.0.1:9999/push",
                "keys": { "p256dh": pub_b64, "auth": auth_b64 }
            }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/web-push/subscribe")
                    .header("content-type", "application/json")
                    .body(Body::from(ok_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/web-push/subscriptions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let listed: Vec<serde_json::Value> =
            serde_json::from_slice(&to_bytes(resp.into_body(), 64 * 1024).await.unwrap()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["endpoint"], "http://127.0.0.1:9999/push");

        let unsub_body = serde_json::json!({ "endpoint": "http://127.0.0.1:9999/push" });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/web-push/unsubscribe")
                    .header("content-type", "application/json")
                    .body(Body::from(unsub_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
