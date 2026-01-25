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
    http::header,
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
    routing::get,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::Deserialize;
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use utoipa::ToSchema;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::api::error::{ApiError, ApiResult};
use crate::api::proto::log_event::{self, EventType, LogLevel};
use crate::api::server::AppState;
use crate::logging::available_modules;

/// Query parameters for WebSocket connection (JWT token).
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    pub token: String,
}

/// Optional authentication token via query parameter.
///
/// This is primarily used for endpoints where setting headers is inconvenient
/// (e.g., links/downloads), and mirrors the WebSocket auth pattern.
#[derive(Debug, Deserialize)]
pub struct OptionalAuthParams {
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct LogRangeQuery {
    /// Inclusive start date in YYYY-MM-DD.
    pub from: Option<String>,
    /// Inclusive end date in YYYY-MM-DD.
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListLogFilesQuery {
    /// Inclusive start date in YYYY-MM-DD.
    pub from: Option<String>,
    /// Inclusive end date in YYYY-MM-DD.
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ArchiveQuery {
    /// Single-use download token issued by `/api/logging/archive-token`.
    pub token: String,
    /// Inclusive start date in YYYY-MM-DD.
    pub from: Option<String>,
    /// Inclusive end date in YYYY-MM-DD.
    pub to: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogFileInfo {
    /// Log date in YYYY-MM-DD.
    pub date: String,
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogFilesResponse {
    pub items: Vec<LogFileInfo>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArchiveTokenResponse {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListLogEntriesQuery {
    /// Optional specific log file name (e.g. rust-srec.log.2026-01-23 or rust-srec.log)
    pub file: Option<String>,
    /// Inclusive start date in YYYY-MM-DD.
    pub from: Option<String>,
    /// Inclusive end date in YYYY-MM-DD.
    pub to: Option<String>,
    /// Filter to only lines containing this substring.
    pub contains: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogEntry {
    pub filename: String,
    pub line_no: u64,
    pub text: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogEntriesResponse {
    pub items: Vec<LogEntry>,
    pub limit: u32,
    pub offset: u32,
    pub has_more: bool,
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
        .route("/files", get(list_log_files))
        .route("/entries", get(list_log_entries))
        .route("/archive-token", get(get_archive_token))
        .route("/archive", get(download_logs_archive))
        // Backwards-compatibility alias
        .route("/download", get(download_logs_archive))
        .route("/stream", get(logging_stream_ws))
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?;
    let value = value.to_str().ok()?;
    let value = value.strip_prefix("Bearer ")?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn validate_token(state: &AppState, token: &str) -> Result<(), ApiError> {
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Authentication not configured"))?;

    jwt_service
        .validate_token(token)
        .map_err(|_| ApiError::unauthorized("Invalid or expired token"))?;

    Ok(())
}

fn parse_yyyy_mm_dd(s: &str) -> Result<chrono::NaiveDate, ApiError> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("Invalid date; expected YYYY-MM-DD"))
}

fn parse_range(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(Option<chrono::NaiveDate>, Option<chrono::NaiveDate>), ApiError> {
    let from = match from {
        Some(s) => Some(parse_yyyy_mm_dd(s)?),
        None => None,
    };
    let to = match to {
        Some(s) => Some(parse_yyyy_mm_dd(s)?),
        None => None,
    };

    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err(ApiError::bad_request(
            "Invalid date range: from must be <= to",
        ));
    }

    Ok((from, to))
}

#[derive(Debug, Clone)]
struct LogFileInternal {
    date: chrono::NaiveDate,
    filename: String,
    path: PathBuf,
    size_bytes: u64,
}

fn scan_log_files(log_dir: &std::path::Path) -> Result<Vec<LogFileInternal>, ApiError> {
    let entries = std::fs::read_dir(log_dir)
        .map_err(|e| ApiError::internal(format!("Failed to read log dir: {e}")))?;

    let mut out = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|e| ApiError::internal(format!("Failed to read dir entry: {e}")))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if !filename.starts_with("rust-srec.log") {
            continue;
        }

        let meta = std::fs::metadata(&path)
            .map_err(|e| ApiError::internal(format!("Failed to stat log file: {e}")))?;
        let size_bytes = meta.len();

        let date = if let Some(suffix) = filename.strip_prefix("rust-srec.log.") {
            chrono::NaiveDate::parse_from_str(suffix, "%Y-%m-%d").ok()
        } else {
            None
        }
        .or_else(|| {
            meta.modified().ok().map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.date_naive()
            })
        })
        .unwrap_or_else(|| chrono::Utc::now().date_naive());

        out.push(LogFileInternal {
            date,
            filename,
            path,
            size_bytes,
        });
    }

    out.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.filename.cmp(&b.filename))
    });
    Ok(out)
}

fn filter_by_range(
    items: Vec<LogFileInternal>,
    from: Option<chrono::NaiveDate>,
    to: Option<chrono::NaiveDate>,
) -> Vec<LogFileInternal> {
    items
        .into_iter()
        .filter(|item| {
            if let Some(from) = from
                && item.date < from
            {
                return false;
            }

            if let Some(to) = to
                && item.date > to
            {
                return false;
            }
            true
        })
        .collect()
}

fn filter_by_file_name(items: Vec<LogFileInternal>, file: &str) -> Vec<LogFileInternal> {
    items.into_iter().filter(|f| f.filename == file).collect()
}

fn list_log_lines(
    files: Vec<LogFileInternal>,
    offset: u64,
    limit: u64,
    contains: Option<&str>,
) -> Result<LogEntriesResponse, ApiError> {
    let mut skipped: u64 = 0;
    let mut collected: Vec<LogEntry> = Vec::new();
    let mut has_more = false;

    let mut remaining = limit + 1; // collect one extra to know has_more

    for file in files {
        let fh = std::fs::File::open(&file.path)
            .map_err(|e| ApiError::internal(format!("Failed to open log file: {e}")))?;
        let mut reader = BufReader::new(fh);

        let mut line = String::new();
        let mut line_no: u64 = 0;

        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .map_err(|e| ApiError::internal(format!("Failed to read log file: {e}")))?;
            if n == 0 {
                break;
            }
            line_no += 1;

            let mut text = line.as_str();
            if text.ends_with('\n') {
                text = &text[..text.len() - 1];
                if text.ends_with('\r') {
                    text = &text[..text.len() - 1];
                }
            }

            if let Some(needle) = contains
                && !text.contains(needle)
            {
                continue;
            }

            if skipped < offset {
                skipped += 1;
                continue;
            }

            if remaining == 0 {
                has_more = true;
                break;
            }

            remaining -= 1;
            collected.push(LogEntry {
                filename: file.filename.clone(),
                line_no,
                text: text.to_string(),
            });

            if remaining == 0 {
                has_more = true;
                break;
            }
        }

        if has_more {
            break;
        }
    }

    if has_more {
        collected.truncate(limit as usize);
    }

    Ok(LogEntriesResponse {
        items: collected,
        limit: limit as u32,
        offset: offset as u32,
        has_more,
    })
}

fn build_archive_zip(files: &[LogFileInternal]) -> Result<Vec<u8>, ApiError> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for f in files {
        zip.start_file(&f.filename, options)
            .map_err(|e| ApiError::internal(format!("Failed to add zip entry: {e}")))?;

        let mut file = std::fs::File::open(&f.path)
            .map_err(|e| ApiError::internal(format!("Failed to open log file: {e}")))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| ApiError::internal(format!("Failed to read log file: {e}")))?;
        zip.write_all(&buf)
            .map_err(|e| ApiError::internal(format!("Failed to write zip entry: {e}")))?;
    }

    zip.finish()
        .map_err(|e| ApiError::internal(format!("Failed to finish zip: {e}")))?;

    Ok(cursor.into_inner())
}

fn format_archive_filename(
    from: Option<chrono::NaiveDate>,
    to: Option<chrono::NaiveDate>,
) -> String {
    match (from, to) {
        (Some(from), Some(to)) if from == to => {
            format!("rust-srec-logs-{}.zip", from.format("%Y-%m-%d"))
        }
        (Some(from), Some(to)) => format!(
            "rust-srec-logs-{}-to-{}.zip",
            from.format("%Y-%m-%d"),
            to.format("%Y-%m-%d")
        ),
        _ => format!(
            "rust-srec-logs-{}.zip",
            chrono::Utc::now().format("%Y-%m-%d")
        ),
    }
}

fn generate_download_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn cleanup_expired_download_tokens(state: &AppState) {
    if state.logging_download_tokens.len() < 1000 {
        return;
    }
    let now = chrono::Utc::now();
    state
        .logging_download_tokens
        .retain(|_, expires_at| *expires_at > now);
}

fn issue_download_token(state: &AppState) -> Result<ArchiveTokenResponse, ApiError> {
    cleanup_expired_download_tokens(state);

    let token = generate_download_token();
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);
    state
        .logging_download_tokens
        .insert(token.clone(), expires_at);

    Ok(ArchiveTokenResponse {
        token,
        expires_at: expires_at.to_rfc3339(),
    })
}

fn consume_download_token(state: &AppState, token: &str) -> Result<(), ApiError> {
    let now = chrono::Utc::now();
    match state.logging_download_tokens.remove(token) {
        Some((_, expires_at)) if expires_at > now => Ok(()),
        _ => Err(ApiError::unauthorized("Invalid or expired download token")),
    }
}

#[utoipa::path(
    get,
    path = "/api/logging/files",
    tag = "logging",
    params(ListLogFilesQuery),
    responses(
        (status = 200, description = "Log files", body = LogFilesResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 400, description = "Invalid query", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_log_files(
    State(state): State<AppState>,
    Query(query): Query<ListLogFilesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<LogFilesResponse>> {
    let token =
        extract_bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing token"))?;
    validate_token(&state, &token)?;

    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;
    let log_dir = logging_config.log_dir().to_path_buf();

    let (from, to) = parse_range(query.from.as_deref(), query.to.as_deref())?;
    let limit = query.limit.unwrap_or(50).min(500);
    let offset = query.offset.unwrap_or(0);

    let result = tokio::task::spawn_blocking(move || {
        let files = scan_log_files(&log_dir)?;
        let files = filter_by_range(files, from, to);
        let total = files.len() as u64;
        let items = files
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|f| LogFileInfo {
                date: f.date.format("%Y-%m-%d").to_string(),
                filename: f.filename,
                size_bytes: f.size_bytes,
            })
            .collect::<Vec<_>>();

        Ok::<_, ApiError>(LogFilesResponse {
            items,
            total,
            limit,
            offset,
        })
    })
    .await
    .map_err(|e| ApiError::internal(format!("Failed to join list task: {e}")))??;

    Ok(Json(result))
}

#[utoipa::path(
    get,
    path = "/api/logging/archive-token",
    tag = "logging",
    params(LogRangeQuery),
    responses(
        (status = 200, description = "Archive token", body = ArchiveTokenResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_archive_token(
    State(state): State<AppState>,
    Query(_range): Query<LogRangeQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<ArchiveTokenResponse>> {
    let token =
        extract_bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing token"))?;
    validate_token(&state, &token)?;
    Ok(Json(issue_download_token(&state)?))
}

#[utoipa::path(
    get,
    path = "/api/logging/entries",
    tag = "logging",
    params(ListLogEntriesQuery),
    responses(
        (status = 200, description = "Log lines", body = LogEntriesResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 400, description = "Invalid query", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_log_entries(
    State(state): State<AppState>,
    Query(query): Query<ListLogEntriesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<LogEntriesResponse>> {
    let token =
        extract_bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing token"))?;
    validate_token(&state, &token)?;

    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;
    let log_dir = logging_config.log_dir().to_path_buf();

    let limit = query.limit.unwrap_or(200).min(2000) as u64;
    let offset = query.offset.unwrap_or(0) as u64;

    let file = query.file.clone();
    let contains = query.contains.clone();
    let from = query.from.clone();
    let to = query.to.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut files = scan_log_files(&log_dir)?;

        if let Some(file) = file.as_deref() {
            // Ensure callers can't traverse out of the log directory.
            if file.contains('/') || file.contains('\\') {
                return Err(ApiError::bad_request("Invalid file name"));
            }
            if !file.starts_with("rust-srec.log") {
                return Err(ApiError::bad_request("Invalid file name"));
            }
            files = filter_by_file_name(files, file);
        } else {
            let (from, to) = parse_range(from.as_deref(), to.as_deref())?;
            files = filter_by_range(files, from, to);
        }

        list_log_lines(files, offset, limit, contains.as_deref())
    })
    .await
    .map_err(|e| ApiError::internal(format!("Failed to join entries task: {e}")))??;

    Ok(Json(result))
}

#[utoipa::path(
    get,
    path = "/api/logging/archive",
    tag = "logging",
    params(ArchiveQuery),
    responses(
        (status = 200, description = "Zipped log files", content_type = "application/zip"),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 400, description = "Invalid query", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn download_logs_archive(
    State(state): State<AppState>,
    Query(query): Query<ArchiveQuery>,
) -> Result<impl IntoResponse, ApiError> {
    consume_download_token(&state, &query.token)?;

    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;
    let log_dir = logging_config.log_dir().to_path_buf();

    let (from, to) = parse_range(query.from.as_deref(), query.to.as_deref())?;

    let zip_bytes = tokio::task::spawn_blocking(move || {
        let files = scan_log_files(&log_dir)?;
        let files = filter_by_range(files, from, to);
        build_archive_zip(&files)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Failed to join archive task: {e}")))??;

    let filename = format_archive_filename(from, to);

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|e| ApiError::internal(format!("Invalid header value: {e}")))?,
    );

    Ok((response_headers, zip_bytes))
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
    headers: HeaderMap,
) -> ApiResult<Json<LoggingConfigResponse>> {
    let token =
        extract_bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing token"))?;
    validate_token(&state, &token)?;

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
    headers: HeaderMap,
    Json(request): Json<UpdateLogFilterRequest>,
) -> ApiResult<Json<LoggingConfigResponse>> {
    let token =
        extract_bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing token"))?;
    validate_token(&state, &token)?;

    let logging_config = state
        .logging_config
        .as_ref()
        .ok_or_else(|| ApiError::internal("Logging configuration not available"))?;

    // Apply the new filter
    logging_config
        .set_filter(&request.filter)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

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
