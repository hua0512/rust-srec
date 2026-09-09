//! Logging API routes.
//!
//! Provides endpoints to view and modify log configuration,
//! and real-time log streaming via WebSocket using Protocol Buffers.

use axum::{
    Json, Router,
    extract::{
        FromRef, Query, State,
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
use std::path::PathBuf;
use std::time::Duration;
use utoipa::ToSchema;

mod archive;
pub(crate) use archive::LogArchiveService;

use crate::api::auth_request::{
    AccessPolicy, authorize_request, revalidate, run_authenticated_session,
};
use crate::api::auth_service::AuthPrincipal;
use crate::api::error::{ApiError, ApiResult};
use crate::api::proto::log_event::{self, EventType, LogLevel};
use crate::api::server::AppState;
use crate::api::server::LoggingArchiveGrant;
use crate::logging::available_modules;

#[derive(Clone)]
pub struct LoggingRouteState {
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
    logging_config: std::sync::Arc<crate::logging::LoggingConfig>,
    logging_download_tokens: std::sync::Arc<dashmap::DashMap<String, LoggingArchiveGrant>>,
}

impl FromRef<AppState> for LoggingRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
            config_service: state.config_service.clone(),
            logging_config: state.logging_config.clone(),
            logging_download_tokens: state.logging_download_tokens.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ArchiveRouteState {
    log_dir: PathBuf,
    tokens: std::sync::Arc<dashmap::DashMap<String, LoggingArchiveGrant>>,
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    archives: std::sync::Arc<LogArchiveService>,
}

impl FromRef<AppState> for ArchiveRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            log_dir: state.logging_config.log_dir().to_path_buf(),
            tokens: state.logging_download_tokens.clone(),
            auth_service: state.auth_service.clone(),
            archives: state.logging_archives.clone(),
        }
    }
}

/// Query credential fallback for clients that cannot send Authorization headers.
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    pub token: Option<String>,
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

async fn authorize_headers(state: &LoggingRouteState, headers: &HeaderMap) -> Result<(), ApiError> {
    authorize_request(
        state.auth_service.as_ref(),
        headers,
        None,
        AccessPolicy::Full,
    )
    .await
    .map(|_| ())
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
    scan_log_files_matching(log_dir, |_| true, usize::MAX)
}

fn scan_log_files_matching(
    log_dir: &std::path::Path,
    include: impl Fn(&LogFileInternal) -> bool,
    max_files: usize,
) -> Result<Vec<LogFileInternal>, ApiError> {
    let entries = std::fs::read_dir(log_dir).map_err(ApiError::from)?;

    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(ApiError::from)?;
        let path = entry.path();
        if !entry.file_type().map_err(ApiError::from)?.is_file() {
            continue;
        }

        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if !filename.starts_with("rust-srec.log") {
            continue;
        }

        let meta = std::fs::metadata(&path).map_err(ApiError::from)?;
        let size_bytes = meta.len();

        let date = crate::logging::store::managed_log_date(&filename)
            .or_else(|| {
                meta.modified().ok().map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.date_naive()
                })
            })
            .unwrap_or_else(|| chrono::Utc::now().date_naive());

        let file = LogFileInternal {
            date,
            filename,
            path,
            size_bytes,
        };
        if include(&file) {
            if out.len() == max_files {
                return Err(ApiError::bad_request(
                    "Too many log files for one archive; choose a narrower date range",
                ));
            }
            out.push(file);
        }
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
        let fh = std::fs::File::open(&file.path).map_err(ApiError::from)?;
        let mut reader = BufReader::new(fh);

        let mut line = String::new();
        let mut line_no: u64 = 0;

        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(ApiError::from)?;
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
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn cleanup_expired_download_tokens(tokens: &dashmap::DashMap<String, LoggingArchiveGrant>) {
    if tokens.len() < 1000 {
        return;
    }
    let now = chrono::Utc::now();
    tokens.retain(|_, grant| grant.expires_at > now);
}

fn issue_download_token(
    tokens: &dashmap::DashMap<String, LoggingArchiveGrant>,
    principal: Option<AuthPrincipal>,
) -> Result<ArchiveTokenResponse, ApiError> {
    cleanup_expired_download_tokens(tokens);

    let token = generate_download_token();
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);
    tokens.insert(
        token.clone(),
        LoggingArchiveGrant {
            expires_at,
            principal,
        },
    );

    Ok(ArchiveTokenResponse {
        token,
        expires_at: expires_at.to_rfc3339(),
    })
}

async fn consume_download_token(
    tokens: &dashmap::DashMap<String, LoggingArchiveGrant>,
    token: &str,
    auth_service: Option<&std::sync::Arc<crate::api::auth_service::AuthService>>,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now();
    match tokens.remove(token) {
        Some((_, grant)) if grant.expires_at > now => tokio::time::timeout(
            Duration::from_secs(3),
            revalidate(auth_service, grant.principal.as_ref(), AccessPolicy::Full),
        )
        .await
        .map_err(|_| ApiError::unauthorized("Archive credential revalidation timed out"))?,
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
    State(state): State<LoggingRouteState>,
    Query(query): Query<ListLogFilesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<LogFilesResponse>> {
    authorize_headers(&state, &headers).await?;

    let logging_config = &state.logging_config;
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
    .map_err(ApiError::from)??;

    Ok(Json(result))
}

#[utoipa::path(
    get,
    path = "/api/logging/archive-token",
    tag = "logging",
    responses(
        (status = 200, description = "Archive token", body = ArchiveTokenResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_archive_token(
    State(state): State<LoggingRouteState>,
    headers: HeaderMap,
) -> ApiResult<Json<ArchiveTokenResponse>> {
    let principal = authorize_request(
        state.auth_service.as_ref(),
        &headers,
        None,
        AccessPolicy::Full,
    )
    .await?;
    Ok(Json(issue_download_token(
        &state.logging_download_tokens,
        principal,
    )?))
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
    State(state): State<LoggingRouteState>,
    Query(query): Query<ListLogEntriesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<LogEntriesResponse>> {
    authorize_headers(&state, &headers).await?;

    let logging_config = &state.logging_config;
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
    .map_err(ApiError::from)??;

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
        (status = 429, description = "Archive download capacity exhausted", body = crate::api::error::ApiErrorResponse),
        (status = 400, description = "Invalid query", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn download_logs_archive(
    State(state): State<ArchiveRouteState>,
    Query(query): Query<ArchiveQuery>,
) -> Result<impl IntoResponse, ApiError> {
    consume_download_token(&state.tokens, &query.token, state.auth_service.as_ref()).await?;

    let (from, to) = parse_range(query.from.as_deref(), query.to.as_deref())?;

    let body = state.archives.download(state.log_dir, from, to).await?;

    let filename = format_archive_filename(from, to);

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(ApiError::from)?,
    );

    Ok((response_headers, body))
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
    State(state): State<LoggingRouteState>,
    headers: HeaderMap,
) -> ApiResult<Json<LoggingConfigResponse>> {
    authorize_headers(&state, &headers).await?;

    let logging_config = &state.logging_config;

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
    State(state): State<LoggingRouteState>,
    headers: HeaderMap,
    Json(request): Json<UpdateLogFilterRequest>,
) -> ApiResult<Json<LoggingConfigResponse>> {
    authorize_headers(&state, &headers).await?;

    let logging_config = &state.logging_config;

    // Apply the new filter
    logging_config
        .set_filter(&request.filter)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let mut global_config = state
        .config_service
        .get_global_config()
        .await
        .map_err(ApiError::from)?;
    global_config.log_filter_directive = request.filter.clone();
    state
        .config_service
        .update_global_config(&global_config)
        .await
        .map_err(ApiError::from)?;

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
    State(state): State<LoggingRouteState>,
    Query(auth): Query<WsAuthParams>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authorize_request(
        state.auth_service.as_ref(),
        &headers,
        auth.token.as_deref(),
        AccessPolicy::Full,
    )
    .await?;

    let logging_config = state.logging_config.clone();

    Ok(ws.on_upgrade(move |socket| {
        run_authenticated_session(
            state.auth_service,
            principal,
            AccessPolicy::Full,
            handle_socket(socket, logging_config),
        )
    }))
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
mod archive_tests;

#[cfg(test)]
mod auth_tests;

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
