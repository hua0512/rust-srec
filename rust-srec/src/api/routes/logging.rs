//! Logging API routes.
//!
//! Provides endpoints to view and modify log configuration,
//! and real-time log streaming via WebSocket using Protocol Buffers.

use axum::{
    Json, Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::Deserialize;
use std::time::Duration;

use crate::api::error::{ApiError, ApiResult};
use crate::api::proto::log_event::{self, EventType, LogLevel};
use crate::api::server::AppState;
use crate::logging::available_modules;

/// Query parameters for WebSocket connection (JWT token).
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    pub token: String,
}

/// Request to update the log filter.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateLogFilterRequest {
    pub filter: String,
}

/// Response for logging configuration.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct LoggingConfigResponse {
    pub filter: String,
    pub available_modules: Vec<ModuleInfo>,
}

/// Information about an available logging module.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ModuleInfo {
    pub name: String,
    pub description: String,
}

/// Create the logging router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_logging_config).put(update_logging_config))
        .route("/stream", get(logging_stream_ws))
}

#[utoipa::path(
    get,
    path = "/api/logging",
    tag = "logging",
    responses(
        (status = 200, description = "Logging configuration", body = LoggingConfigResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_logging_config(
    State(state): State<AppState>,
) -> ApiResult<Json<LoggingConfigResponse>> {
    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;

    let filter = logging_config.get_filter();
    let modules: Vec<ModuleInfo> = available_modules()
        .into_iter()
        .map(|(name, desc)| ModuleInfo {
            name: name.to_string(),
            description: desc.to_string(),
        })
        .collect();

    Ok(Json(LoggingConfigResponse {
        filter,
        available_modules: modules,
    }))
}

#[utoipa::path(
    put,
    path = "/api/logging",
    tag = "logging",
    request_body = UpdateLogFilterRequest,
    responses(
        (status = 200, description = "Logging configuration updated", body = LoggingConfigResponse),
        (status = 400, description = "Invalid filter", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_logging_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateLogFilterRequest>,
) -> ApiResult<Json<LoggingConfigResponse>> {
    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;

    // Apply the new filter
    logging_config
        .set_filter(&request.filter)
        .map_err(|e| ApiError::bad_request(format!("Invalid filter: {}", e)))?;

    // Persist to database if config service is available
    if let Some(config_service) = &state.config_service {
        let mut global_config = config_service
            .get_global_config()
            .await
            .map_err(|e| ApiError::internal(format!("Failed to get config: {}", e)))?;

        global_config.log_filter_directive = request.filter.clone();

        config_service
            .update_global_config(&global_config)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to persist config: {}", e)))?;
    }

    let modules: Vec<ModuleInfo> = available_modules()
        .into_iter()
        .map(|(name, desc)| ModuleInfo {
            name: name.to_string(),
            description: desc.to_string(),
        })
        .collect();

    Ok(Json(LoggingConfigResponse {
        filter: request.filter,
        available_modules: modules,
    }))
}

/// WebSocket handler for real-time log streaming.
async fn logging_stream_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(auth): Query<WsAuthParams>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate JWT token
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Authentication not configured"))?;

    jwt_service
        .validate_token(&auth.token)
        .map_err(|_| ApiError::unauthorized("Invalid or expired token"))?;

    let logging_config = state
        .logging_config
        .clone()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, logging_config)))
}

/// Handle an established WebSocket connection for log streaming.
async fn handle_socket(
    socket: WebSocket,
    logging_config: std::sync::Arc<crate::logging::LoggingConfig>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut log_rx = logging_config.subscribe();

    // Heartbeat interval
    let heartbeat_interval = Duration::from_secs(30);
    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Receive log events and send to client as protobuf binary
            result = log_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Convert internal LogEvent to protobuf LogEvent
                        let proto_event = log_event::LogEvent {
                            timestamp_ms: event.timestamp.timestamp_millis(),
                            level: parse_log_level(&event.level) as i32,
                            target: event.target,
                            message: event.message,
                        };
                        let ws_msg = log_event::WsMessage {
                            event_type: EventType::Log as i32,
                            payload: Some(log_event::ws_message::Payload::Log(proto_event)),
                        };
                        let bytes = ws_msg.encode_to_vec();
                        if sender.send(Message::Binary(Bytes::from(bytes))).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Client is too slow, skip some events
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break; // Channel closed
                    }
                }
            }

            // Send heartbeat pings
            _ = heartbeat.tick() => {
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break; // Client disconnected
                }
            }

            // Handle incoming messages from client
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(_)) => continue, // Ignore other messages
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

/// Parse a log level string to protobuf LogLevel enum.
fn parse_log_level(level: &str) -> LogLevel {
    match level.to_uppercase().as_str() {
        "TRACE" => LogLevel::Trace,
        "DEBUG" => LogLevel::Debug,
        "INFO" => LogLevel::Info,
        "WARN" | "WARNING" => LogLevel::Warn,
        "ERROR" => LogLevel::Error,
        _ => LogLevel::Unspecified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_request_deserialize() {
        let json = r#"{"filter": "rust_srec=debug"}"#;
        let request: UpdateLogFilterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.filter, "rust_srec=debug");
    }

    #[test]
    fn test_logging_config_response_serialize() {
        let response = LoggingConfigResponse {
            filter: "rust_srec=info".to_string(),
            available_modules: vec![ModuleInfo {
                name: "rust_srec".to_string(),
                description: "Main app".to_string(),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("rust_srec=info"));
    }
}
