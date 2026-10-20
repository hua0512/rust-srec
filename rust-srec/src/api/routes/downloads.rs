//! Download progress WebSocket routes.
//!
//! Provides real-time download progress streaming via WebSocket connections.
//! Uses Protocol Buffers for efficient binary message encoding.

use std::time::Duration;

use axum::{
    Router,
    extract::{
        FromRef, Query, State,
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

/// Send a snapshot on subscribe/unsubscribe to support mid-join and filtering.
///
/// This is intentionally "best effort": if the client is already gone we just exit.
const SNAPSHOT_ON_SUBSCRIBE: bool = true;

use crate::api::error::ApiError;
use crate::api::proto::SnapshotJobs;
use crate::api::proto::{
    ClientMessage, DownloadCancelled, DownloadCompleted, DownloadFailed, DownloadRejected,
    EventType, SegmentCompleted, StreamerCheckRecorded, WsMessage, create_snapshot_message,
    download_progress::client_message::Action, download_progress::ws_message::Payload,
    upload_progress_to_proto,
};
use crate::api::server::AppState;
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::domain::streamer::{CheckOutcome, CheckRecord};
use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent};
use crate::pipeline::{PipelineManager, UploadStatusEvent, UploadTerminalStatus};

#[derive(Clone)]
pub struct DownloadRouteState {
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    download_manager: std::sync::Arc<crate::downloader::DownloadManager>,
    check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster,
    /// Source of the snapshot's `uploads` and `jobs` slices (`list_active_jobs`).
    pipeline_manager: std::sync::Arc<PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>>,
    streamer_avatar: StreamerAvatarLookup,
    credential_blocks: std::sync::Arc<crate::credentials::CredentialBlocks>,
    /// Wakes when the accounts needing attention may have changed.
    credential_attention: tokio::sync::watch::Receiver<u64>,
}

/// Streamer id to avatar URL. Jobs carry the streamer's name but not its
/// avatar, so the API layer adds it from the streamer manager's cache.
pub type StreamerAvatarLookup = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn streamer_avatar_lookup(
    streamer_manager: std::sync::Arc<crate::streamer::StreamerManager<SqlxStreamerRepository>>,
) -> StreamerAvatarLookup {
    std::sync::Arc::new(move |streamer_id| {
        streamer_manager
            .get_streamer(streamer_id)
            .and_then(|streamer| streamer.avatar_url)
            .filter(|url| !url.is_empty())
    })
}

impl FromRef<AppState> for DownloadRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
            download_manager: state.download_manager.clone(),
            check_history_broadcaster: state.check_history_broadcaster.clone(),
            upload_status_broadcaster: state.upload_status_broadcaster.clone(),
            pipeline_manager: state.pipeline_manager.clone(),
            streamer_avatar: streamer_avatar_lookup(state.streamer_manager.clone()),
            credential_blocks: state.services.credential_blocks.clone(),
            credential_attention: state.credential_profiles.attention_changes(),
        }
    }
}

/// Query credential fallback for clients that cannot send Authorization headers.
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    /// Session JWT or full-access API key.
    pub token: Option<String>,
}

/// Create the downloads router.
pub fn router() -> Router<AppState> {
    Router::new().route("/ws", get(download_progress_ws)).route(
        "/{download_id}/split",
        axum::routing::post(request_manual_split),
    )
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ManualSplitResponse {
    request_id: String,
    status: &'static str,
}

#[utoipa::path(
    post,
    path = "/api/downloads/{download_id}/split",
    params(("download_id" = String, Path, description = "Current recording attempt ID")),
    responses(
        (status = 202, description = "Cut requested; completion is delivered over the download WebSocket", body = ManualSplitResponse),
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Full access required"),
        (status = 404, description = "Recording attempt no longer exists"),
        (status = 409, description = "Recording is stopping or lossless cutting is unsupported")
    ),
    security(("bearer_auth" = [])),
    tag = "downloads"
)]
pub async fn request_manual_split(
    State(state): State<DownloadRouteState>,
    axum::extract::Path(download_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<(axum::http::StatusCode, axum::Json<ManualSplitResponse>), ApiError> {
    use crate::api::auth_request::{AccessPolicy, authorize_request};
    use axum::http::StatusCode;

    authorize_request(
        state.auth_service.as_ref(),
        &headers,
        None,
        AccessPolicy::Full,
    )
    .await?;
    let split = state
        .download_manager
        .request_manual_split(&download_id)
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(ManualSplitResponse {
            request_id: split.request_id.to_string(),
            status: split.status.as_str(),
        }),
    ))
}

/// WebSocket handler for download status streaming.
///
/// Authenticates the request, then upgrades to WebSocket.
/// Sends an initial snapshot of active downloads, then streams metadata + metrics deltas.
///
/// # Authentication
/// Accepts a session JWT or full-access API key in the Authorization header,
/// falling back to `?token=<credential>` only when that header is absent.
///
/// # Events (Protocol Buffer encoded)
/// - `snapshot`: Initial list of all active downloads (each entry includes `meta` + `metrics`)
/// - `download_meta`: Low-frequency metadata updates (includes full `download_url`)
/// - `download_metrics`: High-frequency numeric progress updates
/// - `segment_completed`: Segment completed (path, size, duration)
/// - `download_completed`: Download finished successfully
/// - `download_failed`: Download failed
/// - `download_cancelled`: Download cancelled
/// - `download_rejected`: Download rejected before start
///
/// # Client Messages (Protocol Buffer encoded)
/// - `subscribe`: Filter updates to specific streamer_id
/// - `unsubscribe`: Receive all updates (remove filter)
async fn download_progress_ws(
    ws: WebSocketUpgrade,
    State(state): State<DownloadRouteState>,
    Query(auth): Query<WsAuthParams>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    use crate::api::auth_request::{AccessPolicy, authorize_request, run_authenticated_session};
    let principal = authorize_request(
        state.auth_service.as_ref(),
        &headers,
        auth.token.as_deref(),
        AccessPolicy::Full,
    )
    .await?;
    Ok(ws.on_upgrade(move |socket| {
        let service = state.auth_service.clone();
        run_authenticated_session(
            service,
            principal,
            AccessPolicy::Full,
            handle_socket(socket, state),
        )
    }))
}

/// All active upload and long jobs and the number of uploads waiting for a
/// worker, for the snapshot. Never narrowed by the connection's streamer
/// subscription: the client keeps one global store of them (the header's
/// indicators) and replaces it from every snapshot, so a filtered slice
/// would drop other streamers' jobs.
/// Best-effort: a repository error degrades to an empty slice / zero rather
/// than failing the snapshot — download state is still worth delivering.
async fn snapshot_jobs(
    pipeline_manager: &PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>,
    streamer_avatar: &StreamerAvatarLookup,
) -> SnapshotJobs {
    let active = pipeline_manager
        .list_active_jobs()
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to list active jobs for WS snapshot: {}", e);
            Vec::new()
        });
    let pending_uploads = pipeline_manager
        .count_pending_uploads()
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to count pending uploads for WS snapshot: {}", e);
            0
        });
    let streamer_avatars = active
        .iter()
        .filter_map(|job| job.streamer_id.as_deref())
        .filter_map(|id| Some((id.to_string(), streamer_avatar(id)?)))
        .collect();
    SnapshotJobs {
        active,
        pending_uploads,
        streamer_avatars,
    }
}

/// Handle an established WebSocket connection.
async fn handle_socket(socket: WebSocket, state: DownloadRouteState) {
    // debug!("New WebSocket connection established");
    let download_manager = state.download_manager.clone();

    let (mut sender, mut receiver) = socket.split();

    // 1. Subscribe to the broadcasts BEFORE building the snapshot: receivers
    // buffer from subscription time, so events that fire while the snapshot
    // is assembled are delivered afterwards instead of being lost in the
    // snapshot/subscribe gap (the snapshot is the only recovery path for a
    // missed UploadStarted).
    let mut event_rx = download_manager.subscribe_shared();
    let mut check_history_rx = state.check_history_broadcaster.subscribe();
    let mut upload_rx = state.upload_status_broadcaster.subscribe();
    let mut block_rx = state.credential_blocks.subscribe();
    let mut blocks_open = true;
    // Only changes after connecting matter: the client fetches the current
    // list when it mounts and after every snapshot.
    let mut attention_rx = state.credential_attention.clone();
    attention_rx.mark_unchanged();
    let mut attention_open = true;

    // 2. Send initial snapshot as protobuf binary
    let downloads = download_manager.get_active_downloads();
    let queued = download_manager.snapshot_pending();
    let jobs = snapshot_jobs(&state.pipeline_manager, &state.streamer_avatar).await;
    let snapshot_msg = create_snapshot_message(downloads, queued, jobs);
    let bytes = snapshot_msg.encode_to_vec();

    if sender
        .send(Message::Binary(Bytes::from(bytes)))
        .await
        .is_err()
    {
        debug!("Failed to send initial snapshot, client disconnected");
        return;
    }

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

                                         // Snapshot-on-subscribe: ensures a mid-join client gets
                                         // current state even if it missed previous delta events.
                                        if SNAPSHOT_ON_SUBSCRIBE {
                                            let mut downloads = download_manager.get_active_downloads();
                                            let mut queued = download_manager.snapshot_pending();
                                            if let Some(ref streamer_id) = filter {
                                                downloads.retain(|d| &d.streamer_id == streamer_id);
                                                queued.retain(|q| &q.streamer_id == streamer_id);
                                            }
                                            let jobs = snapshot_jobs(&state.pipeline_manager, &state.streamer_avatar).await;
                                            let snapshot_msg = create_snapshot_message(downloads, queued, jobs);
                                            let bytes = snapshot_msg.encode_to_vec();
                                            if sender.send(Message::Binary(Bytes::from(bytes))).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    Some(Action::Unsubscribe(_)) => {
                                        // debug!("Client unsubscribed from filter");
                                        filter = None;

                                        if SNAPSHOT_ON_SUBSCRIBE {
                                            let downloads = download_manager.get_active_downloads();
                                            let queued = download_manager.snapshot_pending();
                                            let jobs = snapshot_jobs(&state.pipeline_manager, &state.streamer_avatar).await;
                                            let snapshot_msg = create_snapshot_message(downloads, queued, jobs);
                                            let bytes = snapshot_msg.encode_to_vec();
                                            if sender.send(Message::Binary(Bytes::from(bytes))).await.is_err() {
                                                break;
                                            }
                                        }
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
                    Some(Ok(Message::Ping(data)))
                        if sender.send(Message::Pong(data.clone())).await.is_err() =>
                    {
                        break;
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
                        // V2 message (metadata/metrics split)
                        if let Some(bytes) = shared_event_bytes(&event, &filter) {
                            match sender.send(Message::Binary(bytes)).await {
                                Ok(_) => {}
                                Err(e) => {
                                    debug!("Failed to send message, client may be slow: {}", e);
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

            // Per-poll check-history events. Filtered against the same
            // `streamer_id` subscription state as download events so a
            // client watching streamer A doesn't see streamer B's bars.
            envelope = check_history_rx.recv() => {
                match envelope {
                    Ok(envelope) => {
                        if filter.as_ref().is_none_or(|f| f == &envelope.record.streamer_id) {
                            // `ws_bytes` was encoded once in the drain task and
                            // refcounts cheaply across subscribers — no
                            // re-encode in this hot path.
                            if let Err(e) = sender
                                .send(Message::Binary(envelope.ws_bytes.clone()))
                                .await
                            {
                                debug!("Failed to send check-history message: {}", e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Lag here is fine — the client's REST refetch
                        // (or page-open snapshot) reconciles missed bars.
                        warn!("check-history broadcast lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // The keepalive sender prevents this in test
                        // harnesses; production loses bars only if the
                        // writer task itself exits, which only happens on
                        // shutdown — fall through to heartbeat / client
                        // close.
                        debug!("check-history broadcast closed");
                    }
                }
            }

            // Upload and long-job events (started/progress/terminal),
            // pre-encoded once by the UploadStatusBroadcaster's encoder.
            // Unlike download events these ignore the streamer subscription,
            // for the same reason as `snapshot_jobs`.
            envelope = upload_rx.recv() => {
                match envelope {
                    Ok(envelope) => {
                        if let Err(e) = sender
                            .send(Message::Binary(envelope.ws_bytes.clone()))
                            .await
                        {
                            debug!("Failed to send upload status message: {}", e);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Lag is tolerable: progress is periodic, and a missed
                        // terminal event is reconciled by the snapshot sent on
                        // the next subscribe/unsubscribe or by the frontend's
                        // staleness guard on its uploads store.
                        warn!("upload-status broadcast lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("upload-status broadcast closed");
                    }
                }
            }

            // Credential block transitions, narrowed like download events.
            change = block_rx.recv(), if blocks_open => {
                match change {
                    Ok(change) => {
                        if filter.as_ref().is_none_or(|f| f == &change.streamer_id) {
                            let bytes = map_credential_block_to_protobuf(&change).encode_to_vec();
                            if let Err(e) = sender.send(Message::Binary(Bytes::from(bytes))).await {
                                debug!("Failed to send credential block message: {}", e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // The streamer lists' periodic refetch carries the
                        // current blocks.
                        warn!("credential-block broadcast lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("credential-block broadcast closed");
                        blocks_open = false;
                    }
                }
            }

            changed = attention_rx.changed(), if attention_open => {
                match changed {
                    Ok(()) => {
                        let bytes = credential_attention_changed_message().encode_to_vec();
                        if let Err(e) = sender.send(Message::Binary(Bytes::from(bytes))).await {
                            debug!("Failed to send credential attention message: {}", e);
                        }
                    }
                    Err(_) => {
                        debug!("credential attention channel closed");
                        attention_open = false;
                    }
                }
            }

            // Send heartbeat ping every 30 seconds
            _ = heartbeat_interval.tick() => {
                if awaiting_pong {
                    // Client didn't respond to previous Ping - close connection
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

fn shared_event_bytes(
    envelope: &crate::utils::shared_event::SharedEvent<DownloadManagerEvent>,
    filter: &Option<String>,
) -> Option<Bytes> {
    if filter
        .as_ref()
        .is_some_and(|id| envelope.event.streamer_id() != id.as_str())
    {
        return None;
    }
    envelope.encoded(|event| {
        map_event_to_protobuf(event).map(|message| Bytes::from(message.encode_to_vec()))
    })
}

/// Map a DownloadManagerEvent to metadata/metrics split messages.
///
/// Returns None for events that are not broadcast to WebSocket clients.
fn map_event_to_protobuf(event: &DownloadManagerEvent) -> Option<WsMessage> {
    match event {
        DownloadManagerEvent::Progress(DownloadProgressEvent::ManualSplitChanged {
            download_id,
            streamer_id,
            state,
            ..
        }) => Some(WsMessage {
            event_type: EventType::DownloadSplit as i32,
            payload: Some(Payload::DownloadSplit(
                crate::api::proto::download_progress::DownloadSplit {
                    download_id: download_id.clone(),
                    streamer_id: streamer_id.clone(),
                    state: Some(crate::api::proto::manual_split_to_proto(state)),
                },
            )),
        }),
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadQueued {
            streamer_id,
            streamer_name,
            session_id,
            engine_type,
            is_high_priority,
            queued_at_ms,
        }) => {
            let payload = crate::api::proto::DownloadQueued {
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                streamer_name: streamer_name.clone(),
                engine_type: engine_type.as_str().to_string(),
                queued_at_ms: *queued_at_ms,
                is_high_priority: *is_high_priority,
            };
            Some(WsMessage {
                event_type: EventType::DownloadQueued as i32,
                payload: Some(Payload::DownloadQueued(payload)),
            })
        }
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued {
            streamer_id,
            streamer_name,
            session_id,
        }) => {
            let payload = crate::api::proto::DownloadDequeued {
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                streamer_name: streamer_name.clone(),
            };
            Some(WsMessage {
                event_type: EventType::DownloadDequeued as i32,
                payload: Some(Payload::DownloadDequeued(payload)),
            })
        }
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
            download_id,
            streamer_id,
            session_id,
            engine_type,
            cdn_host,
            download_url,
            ..
        }) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let meta = crate::api::proto::DownloadMeta {
                manual_split: None,
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                engine_type: engine_type.as_str().to_string(),
                started_at_ms: now_ms,
                // First meta emission is also the initial "updated" time.
                updated_at_ms: now_ms,
                cdn_host: cdn_host.clone(),
                download_url: download_url.clone(),
            };
            Some(WsMessage {
                event_type: EventType::DownloadMeta as i32,
                payload: Some(Payload::DownloadMeta(meta)),
            })
        }
        DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
            download_id,
            progress,
            status,
            ..
        }) => {
            let metrics = crate::api::proto::DownloadMetrics {
                download_id: download_id.clone(),
                status: status.as_str().to_string(),
                bytes_downloaded: progress.bytes_downloaded,
                duration_secs: progress.duration_secs,
                speed_bytes_per_sec: progress.speed_bytes_per_sec,
                segments_completed: progress.segments_completed,
                media_duration_secs: progress.media_duration_secs,
                playback_ratio: progress.playback_ratio,
            };
            Some(WsMessage {
                event_type: EventType::DownloadMetrics as i32,
                payload: Some(Payload::DownloadMetrics(metrics)),
            })
        }
        DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
            download_id,
            streamer_id,
            session_id,
            segment_path,
            segment_index,
            started_at,
            completed_at,
            duration_secs,
            size_bytes,
            split_reason_code,
            ..
        }) => {
            let payload = SegmentCompleted {
                download_id: download_id.clone(),
                streamer_id: streamer_id.clone(),
                segment_path: segment_path.clone(),
                segment_index: *segment_index,
                duration_secs: *duration_secs,
                size_bytes: *size_bytes,
                session_id: session_id.clone(),
                split_reason: split_reason_code.clone().unwrap_or_default(),
                started_at_ms: started_at
                    .as_ref()
                    .map(chrono::DateTime::timestamp_millis)
                    .unwrap_or_default(),
                completed_at_ms: completed_at.timestamp_millis(),
            };
            Some(WsMessage {
                event_type: EventType::SegmentCompleted as i32,
                payload: Some(Payload::SegmentCompleted(payload)),
            })
        }
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
            download_id,
            streamer_id,
            session_id,
            total_bytes,
            total_duration_secs,
            total_segments,
            file_path: _file_path,
            ..
        }) => {
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
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
            download_id,
            streamer_id,
            session_id,
            error,
            recoverable,
            ..
        }) => {
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
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
            download_id,
            streamer_id,
            session_id,
            cause,
            ..
        }) => {
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
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
            streamer_id,
            session_id,
            reason,
            retry_after_secs,
            ..
        }) => {
            let payload = DownloadRejected {
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                reason: reason.clone(),
                retry_after_secs: retry_after_secs.unwrap_or(0),
                recoverable: true,
            };
            Some(WsMessage {
                event_type: EventType::DownloadRejected as i32,
                payload: Some(Payload::DownloadRejected(payload)),
            })
        }
        _ => None,
    }
}

/// Map a domain [`CheckRecord`] to the WebSocket envelope.
///
/// `pub` so the [`crate::services::container::ServiceContainer`] can wrap
/// it as the [`crate::monitor::check_history_writer::WsEncoder`] handed to
/// the broadcaster. Keeping the encoding here (next to the proto
/// definitions and other download-WS mappers) avoids adding a
/// monitor-layer dependency on `api::proto`.
///
/// proto3 scalars have no nullability — the convention here is "empty
/// string means absent" for fatal_kind / filter_reason / error_message /
/// title / category, and `viewer_count = 0` for absent. The frontend
/// translates those back to nulls; the REST shape continues to use real
/// nullability.
pub fn map_check_record_to_protobuf(record: &CheckRecord) -> WsMessage {
    let mut payload = StreamerCheckRecorded {
        streamer_id: record.streamer_id.clone(),
        checked_at_ms: record.checked_at.timestamp_millis(),
        // proto3 int32 is plenty for millisecond durations (max 2.1B ms ≈
        // 24 days). Saturate rather than wrap.
        duration_ms: i32::try_from(record.duration.num_milliseconds()).unwrap_or(i32::MAX),
        outcome: record.outcome.as_str().to_string(),
        fatal_kind: String::new(),
        filter_reason: String::new(),
        error_message: String::new(),
        streams_extracted: 0,
        stream_selected_json: String::new(),
        title: String::new(),
        category: String::new(),
        viewer_count: 0,
        streams_extracted_json: String::new(),
    };

    match &record.outcome {
        CheckOutcome::Live {
            title,
            category,
            viewer_count,
            candidates,
            selected_stream,
        } => {
            payload.title = title.clone();
            payload.category = category.clone().unwrap_or_default();
            payload.streams_extracted = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
            payload.viewer_count = viewer_count.unwrap_or(0);
            if let Some(s) = selected_stream {
                payload.stream_selected_json = serde_json::to_string(s).unwrap_or_default();
            }
            if !candidates.is_empty() {
                payload.streams_extracted_json =
                    serde_json::to_string(candidates).unwrap_or_default();
            }
        }
        CheckOutcome::Offline => {}
        CheckOutcome::Filtered {
            reason,
            title,
            category,
        } => {
            payload.filter_reason = reason.as_str().to_string();
            payload.title = title.clone();
            payload.category = category.clone().unwrap_or_default();
        }
        CheckOutcome::FatalError { kind } => {
            payload.fatal_kind = kind.as_str().to_string();
        }
        CheckOutcome::TransientError { message } => {
            payload.error_message = message.clone();
        }
    }

    WsMessage {
        event_type: EventType::StreamerCheckRecorded as i32,
        payload: Some(Payload::StreamerCheckRecorded(payload)),
    }
}

/// Map a credential block change to the WebSocket envelope. A lifted block
/// leaves every field but the streamer at its proto3 default.
pub fn map_credential_block_to_protobuf(
    change: &crate::credentials::CredentialBlockChange,
) -> WsMessage {
    let mut payload = crate::api::proto::download_progress::StreamerCredentialBlock {
        streamer_id: change.streamer_id.clone(),
        ..Default::default()
    };
    if let Some(block) = &change.block {
        payload.blocked = true;
        payload.reason = block.reason.as_str().to_owned();
        payload.platform_id = block.platform_id.clone();
        payload.since_ms = block.since.timestamp_millis();
    }
    WsMessage {
        event_type: EventType::StreamerCredentialBlock as i32,
        payload: Some(Payload::StreamerCredentialBlock(payload)),
    }
}

fn credential_attention_changed_message() -> WsMessage {
    WsMessage {
        event_type: EventType::CredentialAttentionChanged as i32,
        payload: Some(Payload::CredentialAttentionChanged(
            crate::api::proto::download_progress::CredentialAttentionChanged {},
        )),
    }
}

/// Map an [`UploadStatusEvent`] to the WebSocket envelope.
///
/// `pub` for the same reason as [`map_check_record_to_protobuf`]: the
/// [`crate::services::container`] wraps it as the
/// [`crate::pipeline::UploadWsEncoder`] handed to the
/// `UploadStatusBroadcaster`, keeping proto knowledge out of the pipeline
/// module. Absent `streamer_id`/`session_id` become empty strings (proto3
/// scalar-default convention, same as check-history fields).
pub fn map_upload_event_to_protobuf(
    event: &UploadStatusEvent,
    streamer_avatar: &StreamerAvatarLookup,
) -> WsMessage {
    match event {
        UploadStatusEvent::Started {
            job_id,
            streamer_id,
            streamer_name,
            session_id,
            uploader,
            files_total,
            started_at_ms,
        } => WsMessage {
            event_type: EventType::UploadStarted as i32,
            payload: Some(Payload::UploadStarted(crate::api::proto::UploadStarted {
                job_id: job_id.clone(),
                streamer_id: streamer_id.clone().unwrap_or_default(),
                session_id: session_id.clone().unwrap_or_default(),
                uploader: uploader.to_string(),
                files_total: *files_total,
                started_at_ms: *started_at_ms,
                streamer_name: streamer_name.clone().unwrap_or_default(),
                streamer_avatar: streamer_id
                    .as_deref()
                    .and_then(|id| streamer_avatar(id))
                    .unwrap_or_default(),
            })),
        },
        UploadStatusEvent::Progress {
            job_id,
            streamer_id,
            snapshot,
        } => WsMessage {
            event_type: EventType::UploadProgress as i32,
            payload: Some(Payload::UploadProgress(upload_progress_to_proto(
                job_id,
                streamer_id.as_deref(),
                snapshot,
            ))),
        },
        UploadStatusEvent::Terminal {
            job_id,
            streamer_id,
            status,
            files_succeeded,
            files_failed,
            files_skipped,
            error,
        } => {
            let proto_status = match status {
                UploadTerminalStatus::Completed => {
                    crate::api::proto::UploadTerminalStatus::Completed
                }
                UploadTerminalStatus::Failed => crate::api::proto::UploadTerminalStatus::Failed,
                UploadTerminalStatus::Cancelled => {
                    crate::api::proto::UploadTerminalStatus::Cancelled
                }
            };
            WsMessage {
                event_type: EventType::UploadTerminal as i32,
                payload: Some(Payload::UploadTerminal(crate::api::proto::UploadTerminal {
                    job_id: job_id.clone(),
                    streamer_id: streamer_id.clone().unwrap_or_default(),
                    status: proto_status as i32,
                    files_succeeded: *files_succeeded,
                    files_failed: *files_failed,
                    files_skipped: *files_skipped,
                    error: error.clone().unwrap_or_default(),
                })),
            }
        }
        UploadStatusEvent::Queue { pending } => WsMessage {
            event_type: EventType::UploadQueue as i32,
            payload: Some(Payload::UploadQueue(crate::api::proto::UploadQueue {
                pending: *pending,
            })),
        },
        UploadStatusEvent::JobStarted {
            job_id,
            streamer_id,
            streamer_name,
            job_type,
            started_at_ms,
        } => WsMessage {
            event_type: EventType::JobStarted as i32,
            payload: Some(Payload::JobStarted(crate::api::proto::JobStarted {
                job_id: job_id.clone(),
                streamer_id: streamer_id.clone().unwrap_or_default(),
                job_type: job_type.clone(),
                started_at_ms: *started_at_ms,
                streamer_name: streamer_name.clone().unwrap_or_default(),
                streamer_avatar: streamer_id
                    .as_deref()
                    .and_then(|id| streamer_avatar(id))
                    .unwrap_or_default(),
            })),
        },
        UploadStatusEvent::JobProgress {
            job_id,
            streamer_id,
            snapshot,
        } => WsMessage {
            event_type: EventType::JobProgress as i32,
            payload: Some(Payload::JobProgress(upload_progress_to_proto(
                job_id,
                streamer_id.as_deref(),
                snapshot,
            ))),
        },
        UploadStatusEvent::JobEnded {
            job_id,
            started_at_ms,
        } => WsMessage {
            event_type: EventType::JobEnded as i32,
            payload: Some(Payload::JobEnded(crate::api::proto::JobEnded {
                job_id: job_id.clone(),
                started_at_ms: *started_at_ms,
            })),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_cut_endpoint_requires_full_access() {
        use crate::database::models::ApiKeyAccessLevel;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use std::sync::Arc;
        use tower::ServiceExt;

        let fixture = crate::api::auth_request::tests::fixture().await;
        let (_, read_key) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "cut-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let (_, full_key) = fixture
            .service
            .create_api_key(&fixture.user_id, "cut-full", ApiKeyAccessLevel::Full, None)
            .await
            .unwrap();
        let state = DownloadRouteState {
            auth_service: Some(fixture.service.clone()),
            download_manager: Arc::new(crate::downloader::DownloadManager::new()),
            check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            pipeline_manager: Arc::new(PipelineManager::new()),
            streamer_avatar: Arc::new(|_| None),
            credential_blocks: Arc::new(crate::credentials::CredentialBlocks::new()),
            credential_attention: tokio::sync::watch::channel(0).1,
        };
        let app = Router::new()
            .route(
                "/{download_id}/split",
                axum::routing::post(request_manual_split),
            )
            .with_state(state);
        for (credential, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some("invalid"), StatusCode::UNAUTHORIZED),
            (Some(read_key.as_str()), StatusCode::FORBIDDEN),
            (Some(full_key.as_str()), StatusCode::NOT_FOUND),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri("/stale-attempt/split");
            if let Some(credential) = credential {
                request = request.header("Authorization", format!("Bearer {credential}"));
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        fixture.pool.close().await;
    }

    /// The frontend dispatches on `event_type`, then reads the payload's
    /// string codes and proto3 defaults.
    #[test]
    fn download_events_map_to_the_frames_the_frontend_decodes() {
        use crate::downloader::engine::{DownloadProgress, DownloadStatus, EngineType};

        let decode = |event: DownloadManagerEvent, event_type: EventType| {
            let message = map_event_to_protobuf(&event).expect("event is broadcast");
            assert_eq!(message.event_type, event_type as i32, "{event:?}");
            message.payload.expect("payload")
        };
        let ids = || {
            (
                "dl-1".to_string(),
                "streamer-1".to_string(),
                "Streamer".to_string(),
                "session-1".to_string(),
            )
        };

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadMeta(meta) = decode(
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                engine_type: EngineType::Ffmpeg,
                cdn_host: "cdn.example.com".to_string(),
                download_url: "https://cdn.example.com/live.flv".to_string(),
            }),
            EventType::DownloadMeta,
        ) else {
            panic!("expected meta");
        };
        assert_eq!(
            (
                meta.download_id.as_str(),
                meta.session_id.as_str(),
                meta.engine_type.as_str()
            ),
            ("dl-1", "session-1", "ffmpeg")
        );

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadMetrics(metrics) = decode(
            DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                status: DownloadStatus::Downloading,
                progress: DownloadProgress {
                    bytes_downloaded: 1_024,
                    segments_completed: 5,
                    ..Default::default()
                },
            }),
            EventType::DownloadMetrics,
        ) else {
            panic!("expected metrics");
        };
        assert_eq!(
            (
                metrics.status.as_str(),
                metrics.bytes_downloaded,
                metrics.segments_completed
            ),
            ("downloading", 1_024, 5)
        );

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::SegmentCompleted(segment) = decode(
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                segment_path: "/rec/segment.ts".to_string(),
                segment_index: 5,
                started_at: None,
                completed_at: chrono::Utc::now(),
                duration_secs: 10.5,
                size_bytes: 1_024_000,
                split_reason_code: None,
                split_reason_details_json: None,
            }),
            EventType::SegmentCompleted,
        ) else {
            panic!("expected segment");
        };
        assert_eq!(
            (
                segment.segment_index,
                segment.size_bytes,
                segment.split_reason.as_str(),
                segment.started_at_ms
            ),
            (5, 1_024_000, "", 0)
        );
        assert!(segment.completed_at_ms > 0);

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadCompleted(completed) = decode(
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                total_bytes: 10_240_000,
                total_duration_secs: 3_600.0,
                total_segments: 360,
                file_path: None,
                engine_signal: crate::downloader::EngineEndSignal::Unknown,
                stop_cause: None,
            }),
            EventType::DownloadCompleted,
        ) else {
            panic!("expected completed");
        };
        assert_eq!(
            (completed.total_bytes, completed.total_segments),
            (10_240_000, 360)
        );

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadFailed(failed) = decode(
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                engine_type: EngineType::Mesio,
                protocol: crate::downloader::DownloadProtocol::Hls,
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "Connection timeout".to_string(),
                recoverable: true,
            }),
            EventType::DownloadFailed,
        ) else {
            panic!("expected failed");
        };
        assert_eq!(
            (failed.error.as_str(), failed.recoverable),
            ("Connection timeout", true)
        );

        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadCancelled(cancelled) = decode(
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                cause: crate::downloader::DownloadStopCause::User,
            }),
            EventType::DownloadCancelled,
        ) else {
            panic!("expected cancelled");
        };
        assert_eq!(cancelled.cause, "user");

        let (_, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadRejected(rejected) = decode(
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
                streamer_id,
                streamer_name,
                session_id,
                reason: "Circuit breaker open".to_string(),
                retry_after_secs: Some(60),
                kind: crate::downloader::DownloadRejectedKind::CircuitBreaker,
            }),
            EventType::DownloadRejected,
        ) else {
            panic!("expected rejected");
        };
        assert_eq!(
            (
                rejected.session_id.as_str(),
                rejected.retry_after_secs,
                rejected.recoverable
            ),
            ("session-1", 60, true)
        );

        let control = pipeline_common::ManualSplitControl::default();
        control.enable();
        let state = control.request(Duration::from_secs(30)).unwrap();
        let (download_id, streamer_id, streamer_name, session_id) = ids();
        let Payload::DownloadSplit(split) = decode(
            DownloadManagerEvent::Progress(DownloadProgressEvent::ManualSplitChanged {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                state: state.clone(),
            }),
            EventType::DownloadSplit,
        ) else {
            panic!("expected cut state");
        };
        let split_state = split.state.unwrap();
        assert_eq!(
            (
                split.download_id.as_str(),
                split_state.request_id,
                split_state.revision,
                split_state.status.as_str()
            ),
            ("dl-1", state.request_id, state.revision, "pending")
        );
    }

    #[tokio::test]
    async fn download_socket_rejects_read_keys_and_disconnects_revoked_credentials() {
        use crate::api::auth_request::tests::{fixture, open_socket, wait_for_socket_close};
        use crate::database::models::ApiKeyAccessLevel;
        use std::sync::Arc;
        let fixture = fixture().await;
        let (_, read) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "ws-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let (key, full) = fixture
            .service
            .create_api_key(&fixture.user_id, "ws-full", ApiKeyAccessLevel::Full, None)
            .await
            .unwrap();
        let state = DownloadRouteState {
            auth_service: Some(fixture.service.clone()),
            download_manager: Arc::new(crate::downloader::DownloadManager::new()),
            check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            pipeline_manager: Arc::new(PipelineManager::new()),
            streamer_avatar: Arc::new(|_| None),
            credential_blocks: Arc::new(crate::credentials::CredentialBlocks::new()),
            credential_attention: tokio::sync::watch::channel(0).1,
        };
        let app = Router::new()
            .route("/ws", get(download_progress_ws))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        open_socket(address, &format!("/ws?token={read}"), None, 403).await;
        open_socket(
            address,
            &format!("/ws?token={full}"),
            Some("Basic invalid"),
            401,
        )
        .await;
        let key_socket = open_socket(address, &format!("/ws?token={full}"), None, 101).await;
        let jwt_socket = open_socket(
            address,
            "/ws?token=invalid",
            Some(&format!("Bearer {}", fixture.access_token)),
            101,
        )
        .await;
        fixture
            .service
            .revoke_api_key(&fixture.user_id, &key.id)
            .await
            .unwrap();
        fixture
            .service
            .logout(&fixture.refresh_token)
            .await
            .unwrap();
        tokio::join!(
            wait_for_socket_close(key_socket),
            wait_for_socket_close(jwt_socket)
        );
        drop(server);
        fixture.pool.close().await;
    }

    #[test]
    fn shared_encoding_filters_before_caching_and_reuses_the_wire_payload() {
        let envelope = crate::utils::shared_event::SharedEvent::new(
            DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
                download_id: "download".to_string(),
                streamer_id: "selected".to_string(),
                streamer_name: "Streamer".to_string(),
                session_id: "session".to_string(),
                status: crate::downloader::engine::DownloadStatus::Downloading,
                progress: crate::downloader::engine::DownloadProgress::default(),
            }),
        );
        assert!(shared_event_bytes(&envelope, &Some("other".to_string())).is_none());
        let selected = shared_event_bytes(&envelope, &Some("selected".to_string())).unwrap();
        let unfiltered = shared_event_bytes(&envelope, &None).unwrap();
        assert_eq!(selected.as_ptr(), unfiltered.as_ptr());
        assert_eq!(
            selected.as_ref(),
            map_event_to_protobuf(&envelope.event)
                .unwrap()
                .encode_to_vec()
        );
        assert!(shared_event_bytes(&envelope, &Some("other".to_string())).is_none());
    }

    /// The header's upload status keeps one global uploads list, so a socket
    /// that a page has narrowed to one streamer must still carry every
    /// streamer's uploads, names and avatars included.
    #[tokio::test]
    async fn streamer_filtered_socket_still_receives_other_streamers_uploads() {
        use crate::api::proto::download_progress::SubscribeRequest;
        use std::sync::Arc;
        use tokio_tungstenite::tungstenite::Message as Frame;

        let streamer_avatar: StreamerAvatarLookup =
            Arc::new(|id| (id == "other").then(|| "https://example.com/other.png".to_string()));
        let upload_status_broadcaster = {
            let streamer_avatar = streamer_avatar.clone();
            crate::pipeline::UploadStatusBroadcaster::new(Arc::new(move |event| {
                Bytes::from(map_upload_event_to_protobuf(event, &streamer_avatar).encode_to_vec())
            }))
        };
        let state = DownloadRouteState {
            auth_service: None,
            download_manager: Arc::new(crate::downloader::DownloadManager::new()),
            check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            upload_status_broadcaster: upload_status_broadcaster.clone(),
            pipeline_manager: Arc::new(PipelineManager::new()),
            streamer_avatar,
            credential_blocks: Arc::new(crate::credentials::CredentialBlocks::new()),
            credential_attention: tokio::sync::watch::channel(0).1,
        };
        let app = Router::new()
            .route("/ws", get(download_progress_ws))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));

        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        let (mut outgoing, mut incoming) = socket.split();
        let mut next_message = async || {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Frame::Binary(data) = incoming.next().await.unwrap().unwrap() {
                        return WsMessage::decode(data).unwrap();
                    }
                }
            })
            .await
            .expect("socket message must arrive")
        };
        assert!(matches!(
            next_message().await.payload,
            Some(Payload::Snapshot(_))
        ));

        let subscribe = ClientMessage {
            action: Some(Action::Subscribe(SubscribeRequest {
                streamer_id: "watched".to_string(),
            })),
        };
        outgoing
            .send(Frame::Binary(subscribe.encode_to_vec().into()))
            .await
            .unwrap();
        // The subscribe snapshot is sent after the filter is applied, so the
        // upload below is judged against the narrowed subscription.
        assert!(matches!(
            next_message().await.payload,
            Some(Payload::Snapshot(_))
        ));

        upload_status_broadcaster.send(UploadStatusEvent::Started {
            job_id: "job".to_string(),
            streamer_id: Some("other".to_string()),
            streamer_name: Some("Other Streamer".to_string()),
            session_id: None,
            uploader: "rclone",
            files_total: 1,
            started_at_ms: 0,
        });
        let Some(Payload::UploadStarted(started)) = next_message().await.payload else {
            panic!("expected the other streamer's upload");
        };
        assert_eq!(started.streamer_id, "other");
        assert_eq!(started.streamer_name, "Other Streamer");
        assert_eq!(started.streamer_avatar, "https://example.com/other.png");
    }
    /// Block changes follow the streamer subscription like download events,
    /// while the account-attention marker reaches every socket.
    #[tokio::test]
    async fn credential_blocks_follow_the_subscription_and_attention_reaches_every_socket() {
        use crate::api::proto::download_progress::SubscribeRequest;
        use crate::credentials::UnavailableReason;
        use std::sync::Arc;
        use tokio_tungstenite::tungstenite::Message as Frame;

        let blocks = Arc::new(crate::credentials::CredentialBlocks::new());
        let (attention, attention_rx) = tokio::sync::watch::channel(0u64);
        let state = DownloadRouteState {
            auth_service: None,
            download_manager: Arc::new(crate::downloader::DownloadManager::new()),
            check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster::new(Arc::new(
                |_| Bytes::new(),
            )),
            pipeline_manager: Arc::new(PipelineManager::new()),
            streamer_avatar: Arc::new(|_| None),
            credential_blocks: blocks.clone(),
            credential_attention: attention_rx,
        };
        let app = Router::new()
            .route("/ws", get(download_progress_ws))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));

        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        let (mut outgoing, mut incoming) = socket.split();
        let mut next_message = async || {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Frame::Binary(data) = incoming.next().await.unwrap().unwrap() {
                        return WsMessage::decode(data).unwrap();
                    }
                }
            })
            .await
            .expect("socket message must arrive")
        };
        assert!(matches!(
            next_message().await.payload,
            Some(Payload::Snapshot(_))
        ));
        let subscribe = ClientMessage {
            action: Some(Action::Subscribe(SubscribeRequest {
                streamer_id: "watched".to_string(),
            })),
        };
        outgoing
            .send(Frame::Binary(subscribe.encode_to_vec().into()))
            .await
            .unwrap();
        assert!(matches!(
            next_message().await.payload,
            Some(Payload::Snapshot(_))
        ));

        // Another streamer's block is filtered out; the next frame is the
        // watched streamer's.
        blocks.block("other", "platform-a", UnavailableReason::LoginRequired);
        blocks.block("watched", "platform-a", UnavailableReason::ProfilesDisabled);
        let Some(Payload::StreamerCredentialBlock(blocked)) = next_message().await.payload else {
            panic!("expected the watched streamer's block");
        };
        assert_eq!(blocked.streamer_id, "watched");
        assert!(blocked.blocked);
        assert_eq!(blocked.reason, "profiles_disabled");
        assert_eq!(blocked.platform_id, "platform-a");
        assert_eq!(
            blocked.since_ms,
            blocks.get("watched").unwrap().since.timestamp_millis()
        );

        blocks.clear("watched");
        let Some(Payload::StreamerCredentialBlock(lifted)) = next_message().await.payload else {
            panic!("expected the watched streamer's block to lift");
        };
        assert_eq!(lifted.streamer_id, "watched");
        assert!(!lifted.blocked);
        assert!(lifted.reason.is_empty());

        attention.send_modify(|generation| *generation += 1);
        assert!(matches!(
            next_message().await.payload,
            Some(Payload::CredentialAttentionChanged(_))
        ));
    }
}
