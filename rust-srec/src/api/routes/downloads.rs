//! Download progress WebSocket routes.
//!
//! Provides real-time download progress streaming via WebSocket connections.
//! Uses Protocol Buffers for efficient binary message encoding.

use std::time::Duration;

use axum::{
    Router,
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
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Heartbeat ping interval in seconds.
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

use crate::api::error::ApiError;
use crate::api::proto::{
    ClientMessage, DownloadCancelled, DownloadCompleted, DownloadFailed, DownloadRejected,
    DownloadStarted, EventType, SegmentCompleted, WsMessage, create_error_message,
    create_snapshot_message, download_progress::client_message::Action,
    download_progress::ws_message::Payload,
};
use crate::api::server::AppState;
use crate::downloader::DownloadManagerEvent;

/// Query parameters for WebSocket connection (JWT token).
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    /// JWT token for authentication
    pub token: String,
}

/// Create the downloads router.
pub fn router() -> Router<AppState> {
    Router::new().route("/ws", get(download_progress_ws))
}

/// WebSocket handler for download progress streaming.
///
/// Authenticates via JWT token in query parameter, then upgrades to WebSocket.
/// Sends initial snapshot of active downloads, then streams progress events.
///
/// # Authentication
/// Requires valid JWT token via `?token=<jwt>` query parameter.
///
/// # Events (Protocol Buffer encoded)
/// - `snapshot`: Initial list of all active downloads
/// - `download_started`: New download started
/// - `progress`: Download progress update
/// - `download_completed`: Download finished successfully
/// - `download_failed`: Download failed
///
/// # Client Messages (Protocol Buffer encoded)
/// - `subscribe`: Filter updates to specific streamer_id
/// - `unsubscribe`: Receive all updates (remove filter)
async fn download_progress_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(auth): Query<WsAuthParams>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate JWT token using existing JwtService
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Authentication not configured"))?;

    jwt_service
        .validate_token(&auth.token)
        .map_err(|_| ApiError::unauthorized("Invalid or expired token"))?;

    Ok(ws.on_upgrade(|socket| handle_socket(socket, state)))
}

/// Handle an established WebSocket connection.
async fn handle_socket(socket: WebSocket, state: AppState) {
    // debug!("New WebSocket connection established");
    let download_manager = match &state.download_manager {
        Some(dm) => dm.clone(),
        None => {
            // Send error as protobuf and close
            let (mut sender, _) = socket.split();
            let error_msg =
                create_error_message("SERVICE_UNAVAILABLE", "Download manager is not available");
            let bytes = error_msg.encode_to_vec();
            let _ = sender.send(Message::Binary(Bytes::from(bytes))).await;
            let _ = sender.close().await;
            return;
        }
    };

    let (mut sender, mut receiver) = socket.split();

    // 1. Send initial snapshot as protobuf binary
    let downloads = download_manager.get_active_downloads();
    let snapshot_msg = create_snapshot_message(downloads.clone());
    let bytes = snapshot_msg.encode_to_vec();

    if sender
        .send(Message::Binary(Bytes::from(bytes)))
        .await
        .is_err()
    {
        debug!("Failed to send initial snapshot, client disconnected");
        return;
    }
    // debug!("Sent initial snapshot with {} downloads", downloads.len());

    // 2. Subscribe to broadcast
    let mut event_rx = download_manager.subscribe();

    // 3. Track filter state and heartbeat
    let mut filter: Option<String> = None;
    let mut heartbeat_interval =
        tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let mut awaiting_pong = false;

    // 4. Event loop
    loop {
        tokio::select! {
            // Handle incoming client messages (protobuf binary)
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        match ClientMessage::decode(data.as_ref()) {
                            Ok(client_msg) => {
                                match client_msg.action {
                                    Some(Action::Subscribe(req)) => {
                                        // debug!("Client subscribed to streamer: {}", req.streamer_id);
                                        filter = Some(req.streamer_id);
                                    }
                                    Some(Action::Unsubscribe(_)) => {
                                        // debug!("Client unsubscribed from filter");
                                        filter = None;
                                    }
                                    None => {
                                        // debug!("Client message has no action");
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed to decode client message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        // debug!("Client disconnected");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Respond to client Ping with Pong (Requirement 7.4)
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // Client responded to our Ping - reset awaiting_pong state
                        // debug!("Received Pong from client");
                        awaiting_pong = false;
                    }
                    Some(Err(e)) => {
                        debug!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Handle broadcast events - encode as protobuf
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        if let Some(msg) = map_event_to_protobuf(&event, &filter) {
                            let bytes = msg.encode_to_vec();
                            // Use send for backpressure - if buffer is full, drop message
                            match sender.send(Message::Binary(Bytes::from(bytes))).await {
                                Ok(_) => {
                                    // trace!("Sent event {:?} to client", event);
                                }
                                Err(e) => {
                                    debug!("Failed to send message, client may be slow: {}", e);
                                    // Don't break - client might recover
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Broadcast receiver lagged by {} messages", n);
                        // Continue with next event
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Broadcast channel closed");
                        break;
                    }
                }
            }

            // Send heartbeat ping every 30 seconds (Requirement 7.1)
            _ = heartbeat_interval.tick() => {
                if awaiting_pong {
                    // Client didn't respond to previous Ping - close connection (Requirement 7.3)
                    debug!("Client failed to respond to Ping, closing connection");
                    break;
                }
                // Send Ping message
                if sender.send(Message::Ping(Bytes::new())).await.is_ok() {
                    awaiting_pong = true;
                    // debug!("Sent heartbeat Ping to client");
                } else {
                    debug!("Failed to send Ping, closing connection");
                    break;
                }
            }
        }
    }

    // debug!("WebSocket connection closed, cleaning up");
}

/// Map a DownloadManagerEvent to a protobuf WsMessage, applying filter if set.
///
/// Returns None if the event should be filtered out.
fn map_event_to_protobuf(
    event: &DownloadManagerEvent,
    filter: &Option<String>,
) -> Option<WsMessage> {
    // Extract streamer_id from event for filtering
    let event_streamer_id = match event {
        DownloadManagerEvent::DownloadStarted { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::Progress { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::SegmentCompleted { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::SegmentStarted { streamer_id, .. } => streamer_id, // Added
        DownloadManagerEvent::DownloadCompleted { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::DownloadFailed { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::DownloadCancelled { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::ConfigUpdated { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::ConfigUpdateFailed { streamer_id, .. } => streamer_id,
        DownloadManagerEvent::DownloadRejected { streamer_id, .. } => streamer_id,
    };

    // Apply filter
    if let Some(filter_id) = filter
        && event_streamer_id != filter_id
    {
        return None;
    }

    // Map event to protobuf message
    match event {
        DownloadManagerEvent::DownloadStarted {
            download_id,
            streamer_id,
            session_id,
            engine_type,
        } => {
            let payload = DownloadStarted {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                engine_type: engine_type.as_str().to_string(),
                started_at_ms: chrono::Utc::now().timestamp_millis(),
            };
            Some(WsMessage {
                event_type: EventType::DownloadStarted as i32,
                payload: Some(Payload::DownloadStarted(payload)),
            })
        }
        DownloadManagerEvent::Progress {
            download_id,
            streamer_id,
            session_id,
            download_url,
            progress,
        } => {
            let payload = crate::api::proto::DownloadProgress {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                engine_type: String::new(), // Not available in progress event
                status: "Downloading".to_string(),
                bytes_downloaded: progress.bytes_downloaded,
                duration_secs: progress.duration_secs,
                speed_bytes_per_sec: progress.speed_bytes_per_sec,
                segments_completed: progress.segments_completed,
                media_duration_secs: progress.media_duration_secs,
                playback_ratio: progress.playback_ratio,
                started_at_ms: 0, // Not available in progress event
                download_url: download_url.clone(),
            };
            Some(WsMessage {
                event_type: EventType::Progress as i32,
                payload: Some(Payload::Progress(payload)),
            })
        }
        DownloadManagerEvent::SegmentCompleted {
            download_id,
            streamer_id,
            session_id,
            segment_path,
            segment_index,
            duration_secs,
            size_bytes,
        } => {
            let payload = SegmentCompleted {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                segment_path: segment_path.clone(),
                segment_index: *segment_index,
                duration_secs: *duration_secs,
                size_bytes: *size_bytes,
                session_id: session_id.clone(),
            };
            Some(WsMessage {
                event_type: EventType::SegmentCompleted as i32,
                payload: Some(Payload::SegmentCompleted(payload)),
            })
        }
        DownloadManagerEvent::DownloadCompleted {
            download_id,
            streamer_id,
            session_id,
            total_bytes,
            total_duration_secs,
            total_segments,
            file_path: _file_path,
        } => {
            let payload = DownloadCompleted {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                total_bytes: *total_bytes,
                total_duration_secs: *total_duration_secs,
                total_segments: *total_segments,
            };
            Some(WsMessage {
                event_type: EventType::DownloadCompleted as i32,
                payload: Some(Payload::DownloadCompleted(payload)),
            })
        }
        DownloadManagerEvent::DownloadFailed {
            download_id,
            streamer_id,
            session_id,
            error,
            recoverable,
        } => {
            let payload = DownloadFailed {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                error: error.clone(),
                recoverable: *recoverable,
            };
            Some(WsMessage {
                event_type: EventType::DownloadFailed as i32,
                payload: Some(Payload::DownloadFailed(payload)),
            })
        }
        DownloadManagerEvent::DownloadCancelled {
            download_id,
            streamer_id,
            session_id,
            cause,
        } => {
            let payload = DownloadCancelled {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                cause: cause.as_str().to_string(),
            };
            Some(WsMessage {
                event_type: EventType::DownloadCancelled as i32,
                payload: Some(Payload::DownloadCancelled(payload)),
            })
        }
        // Config events are internal, don't broadcast to clients
        DownloadManagerEvent::ConfigUpdated { .. }
        | DownloadManagerEvent::ConfigUpdateFailed { .. }
        // TODO: SegmentStarted ignored for now
        | DownloadManagerEvent::SegmentStarted { .. } => None,
        // DownloadRejected is informational - could optionally broadcast
        // For now, we include it as it provides useful feedback to UI
        DownloadManagerEvent::DownloadRejected {
            streamer_id,
            session_id,
            reason,
            retry_after_secs,
        } => {
            let payload = DownloadRejected {
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                reason: reason.clone(),
                retry_after_secs: retry_after_secs.unwrap_or(0),
                recoverable: true, // Circuit breaker rejections are always recoverable
            };
            Some(WsMessage {
                event_type: EventType::DownloadRejected as i32,
                payload: Some(Payload::DownloadRejected(payload)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::ConfigUpdateType;
    use crate::downloader::engine::EngineType;

    #[test]
    fn test_ws_auth_params_deserialize() {
        let json = r#"{"token": "test-jwt-token"}"#;
        let params: WsAuthParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.token, "test-jwt-token");
    }

    #[test]
    fn test_filter_matches_streamer() {
        let event = DownloadManagerEvent::DownloadStarted {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            engine_type: EngineType::Ffmpeg,
        };

        // No filter - should pass
        let result = map_event_to_protobuf(&event, &None);
        assert!(result.is_some());

        // Matching filter - should pass
        let result = map_event_to_protobuf(&event, &Some("streamer-123".to_string()));
        assert!(result.is_some());

        // Non-matching filter - should be filtered out
        let result = map_event_to_protobuf(&event, &Some("other-streamer".to_string()));
        assert!(result.is_none());
    }

    #[test]
    fn test_config_events_not_broadcast() {
        let event = DownloadManagerEvent::ConfigUpdated {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            update_type: ConfigUpdateType::Cookies,
        };

        let result = map_event_to_protobuf(&event, &None);
        assert!(result.is_none());
    }

    #[test]
    fn test_download_started_event_mapping() {
        let event = DownloadManagerEvent::DownloadStarted {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            engine_type: EngineType::Ffmpeg,
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::DownloadStarted as i32);

        if let Some(Payload::DownloadStarted(payload)) = msg.payload {
            assert_eq!(payload.download_id, "dl-1");
            assert_eq!(payload.streamer_id, "streamer-123");
            assert_eq!(payload.session_id, "session-1");
            assert_eq!(payload.engine_type, "ffmpeg");
        } else {
            panic!("Expected DownloadStarted payload");
        }
    }

    #[test]
    fn test_segment_completed_event_mapping() {
        let event = DownloadManagerEvent::SegmentCompleted {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            segment_path: "/path/to/segment.ts".to_string(),
            segment_index: 5,
            duration_secs: 10.5,
            size_bytes: 1024000,
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::SegmentCompleted as i32);

        if let Some(Payload::SegmentCompleted(payload)) = msg.payload {
            assert_eq!(payload.download_id, "dl-1");
            assert_eq!(payload.segment_index, 5);
            assert_eq!(payload.duration_secs, 10.5);
            assert_eq!(payload.size_bytes, 1024000);
            assert_eq!(payload.session_id, "session-1");
        } else {
            panic!("Expected SegmentCompleted payload");
        }
    }

    #[test]
    fn test_download_completed_event_mapping() {
        let event = DownloadManagerEvent::DownloadCompleted {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            total_bytes: 10240000,
            total_duration_secs: 3600.0,
            total_segments: 360,
            file_path: Some("/path/to/video.mp4".to_string()),
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::DownloadCompleted as i32);

        if let Some(Payload::DownloadCompleted(payload)) = msg.payload {
            assert_eq!(payload.download_id, "dl-1");
            assert_eq!(payload.total_bytes, 10240000);
            assert_eq!(payload.total_segments, 360);
        } else {
            panic!("Expected DownloadCompleted payload");
        }
    }

    #[test]
    fn test_download_failed_event_mapping() {
        let event = DownloadManagerEvent::DownloadFailed {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            error: "Connection timeout".to_string(),
            recoverable: true,
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::DownloadFailed as i32);

        if let Some(Payload::DownloadFailed(payload)) = msg.payload {
            assert_eq!(payload.download_id, "dl-1");
            assert_eq!(payload.error, "Connection timeout");
            assert!(payload.recoverable);
        } else {
            panic!("Expected DownloadFailed payload");
        }
    }

    #[test]
    fn test_download_cancelled_event_mapping() {
        let event = DownloadManagerEvent::DownloadCancelled {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            cause: crate::downloader::DownloadStopCause::User,
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::DownloadCancelled as i32);

        if let Some(Payload::DownloadCancelled(payload)) = msg.payload {
            assert_eq!(payload.download_id, "dl-1");
            assert_eq!(payload.streamer_id, "streamer-123");
            assert_eq!(payload.cause, "user");
        } else {
            panic!("Expected DownloadCancelled payload");
        }
    }

    #[test]
    fn test_download_rejected_event_mapping() {
        let event = DownloadManagerEvent::DownloadRejected {
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            reason: "Circuit breaker open".to_string(),
            retry_after_secs: Some(60),
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();
        assert_eq!(msg.event_type, EventType::DownloadRejected as i32);

        if let Some(Payload::DownloadRejected(payload)) = msg.payload {
            assert_eq!(payload.streamer_id, "streamer-123");
            assert_eq!(payload.session_id, "session-1");
            assert_eq!(payload.reason, "Circuit breaker open");
            assert_eq!(payload.retry_after_secs, 60);
            assert!(payload.recoverable);
        } else {
            panic!("Expected DownloadRejected payload");
        }
    }

    #[test]
    fn test_protobuf_round_trip_encoding() {
        let event = DownloadManagerEvent::DownloadStarted {
            download_id: "dl-1".to_string(),
            streamer_id: "streamer-123".to_string(),
            session_id: "session-1".to_string(),
            engine_type: EngineType::Ffmpeg,
        };

        let msg = map_event_to_protobuf(&event, &None).unwrap();

        // Encode to bytes
        let bytes = msg.encode_to_vec();

        // Decode back
        let decoded = WsMessage::decode(bytes.as_slice()).unwrap();

        assert_eq!(decoded.event_type, msg.event_type);
        assert!(decoded.payload.is_some());
    }

    #[test]
    fn test_client_message_subscribe_decode() {
        use crate::api::proto::{SubscribeRequest, download_progress::client_message::Action};

        let client_msg = ClientMessage {
            action: Some(Action::Subscribe(SubscribeRequest {
                streamer_id: "streamer-123".to_string(),
            })),
        };

        // Encode
        let bytes = client_msg.encode_to_vec();

        // Decode
        let decoded = ClientMessage::decode(bytes.as_slice()).unwrap();

        if let Some(Action::Subscribe(req)) = decoded.action {
            assert_eq!(req.streamer_id, "streamer-123");
        } else {
            panic!("Expected Subscribe action");
        }
    }

    #[test]
    fn test_client_message_unsubscribe_decode() {
        use crate::api::proto::{UnsubscribeRequest, download_progress::client_message::Action};

        let client_msg = ClientMessage {
            action: Some(Action::Unsubscribe(UnsubscribeRequest {})),
        };

        // Encode
        let bytes = client_msg.encode_to_vec();

        // Decode
        let decoded = ClientMessage::decode(bytes.as_slice()).unwrap();

        assert!(matches!(decoded.action, Some(Action::Unsubscribe(_))));
    }
}
