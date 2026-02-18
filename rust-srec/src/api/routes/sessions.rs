//! Session management routes.
//!
//! This module provides REST API endpoints for querying recording sessions
//! and their associated metadata.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/sessions` | List sessions with filtering and pagination |
//! | GET | `/api/sessions/:id` | Get a single session by ID |

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    DanmuRatePoint, DanmuTopTalker, DanmuWordFrequency, PaginatedResponse, PaginationParams,
    SessionDanmuStatisticsResponse, SessionFilterParams, SessionResponse, TitleChange,
};
use crate::api::server::AppState;
use crate::database::models::{
    DanmuRateEntry, Pagination, SessionFilters, TitleEntry, TopTalkerEntry,
};

/// Create the sessions router.
///
/// # Routes
///
/// - `GET /` - List sessions with filtering and pagination
/// - `GET /:id` - Get a single session by ID
/// - `DELETE /:id` - Delete a single session by ID
/// - `POST /batch-delete` - Delete multiple sessions by IDs
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/batch-delete", post(delete_sessions_batch))
        .route("/{id}/danmu-statistics", get(get_session_danmu_statistics))
        .route("/{id}", get(get_session).delete(delete_session))
}

/// List recording sessions with pagination and filtering.
///
/// # Endpoint
///
/// `GET /api/sessions`
///
/// # Query Parameters
///
/// - `limit` - Maximum number of results (default: 20, max: 100)
/// - `offset` - Number of results to skip (default: 0)
/// - `streamer_id` - Filter by streamer ID
/// - `from_date` - Filter sessions started after this date (ISO 8601)
/// - `to_date` - Filter sessions started before this date (ISO 8601)
/// - `active_only` - If true, return only sessions without an end_time
///
/// # Response
///
/// Returns a paginated list of sessions matching the filter criteria.
///
/// ```json
/// {
///     "items": [
///         {
///             "id": "session-123",
///             "streamer_id": "streamer-456",
///             "streamer_name": "StreamerName",
///             "streamer_avatar": "https://example.com/avatar.jpg",
///             "title": "Stream Title",
///             "titles": [
///                 {"title": "Initial Title", "timestamp": "2025-12-03T10:00:00Z"},
///                 {"title": "Current Stream Title", "timestamp": "2025-12-03T12:00:00Z"}
///             ],
///             "start_time": "2025-12-03T10:00:00Z",
///             "end_time": "2025-12-03T14:00:00Z",
///             "duration_secs": 14400,
///             "output_count": 3,
///             "total_size_bytes": 5368709120,
///             "danmu_count": 15000,
///             "thumbnail_url": "https://example.com/thumbnail.jpg"
///         }
///     ],
///     "total": 50,
///     "limit": 20,
///     "offset": 0
/// }
/// ```
///
#[utoipa::path(
    get,
    path = "/api/sessions",
    tag = "sessions",
    params(PaginationParams, SessionFilterParams),
    responses(
        (status = 200, description = "List of sessions", body = PaginatedResponse<SessionResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<SessionFilterParams>,
) -> ApiResult<Json<PaginatedResponse<SessionResponse>>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    let streamer_repository = state
        .streamer_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Convert API filter params to database filter types
    let db_filters = SessionFilters {
        streamer_id: filters.streamer_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        active_only: filters.active_only,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    // Call SessionRepository.list_sessions_filtered
    let (sessions, total) = session_repository
        .list_sessions_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    // Fetch all streamers for mapping details
    let streamers = streamer_repository
        .list_all_streamers()
        .await
        .map_err(ApiError::from)?;

    let streamer_map: std::collections::HashMap<_, _> =
        streamers.into_iter().map(|s| (s.id.clone(), s)).collect();

    // Convert sessions to API response format
    let mut session_responses: Vec<SessionResponse> = Vec::with_capacity(sessions.len());

    for session in &sessions {
        // Get output count for each session
        let output_count = session_repository
            .get_output_count(&session.id)
            .await
            .unwrap_or(0);

        let start_time = crate::database::time::ms_to_datetime(session.start_time);
        let end_time = session.end_time.map(crate::database::time::ms_to_datetime);

        // Calculate duration
        let duration_secs = end_time.map(|end| (end - start_time).num_seconds() as u64);

        // Parse titles JSON
        let (titles, title) = parse_titles(&session.titles);

        // Get streamer details
        let (streamer_name, streamer_avatar) =
            if let Some(s) = streamer_map.get(&session.streamer_id) {
                (s.name.clone(), s.avatar.clone())
            } else {
                (String::new(), None)
            };

        let danmu_count = session_repository
            .get_danmu_statistics(&session.id)
            .await
            .ok()
            .flatten()
            .map(|stats| stats.total_danmus as u64);

        session_responses.push(SessionResponse {
            id: session.id.clone(),
            streamer_id: session.streamer_id.clone(),
            streamer_name,
            title,
            titles,
            start_time,
            end_time,
            duration_secs,
            output_count,
            total_size_bytes: session.total_size_bytes as u64,
            danmu_count,
            thumbnail_url: get_thumbnail_url(&session.id, session_repository.as_ref()).await,
            streamer_avatar,
        });
    }

    let response =
        PaginatedResponse::new(session_responses, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

/// Get a single session by ID.
///
/// # Endpoint
///
/// `GET /api/sessions/:id`
///
/// # Path Parameters
///
/// - `id` - The session ID (UUID)
///
/// # Response
///
/// Returns the session details including metadata and output count.
///
/// ```json
/// {
///     "id": "session-123",
///     "streamer_id": "streamer-456",
///     "streamer_name": "StreamerName",
///     "title": "Current Stream Title",
///     "titles": [
///         {"title": "Initial Title", "timestamp": "2025-12-03T10:00:00Z"},
///         {"title": "Current Stream Title", "timestamp": "2025-12-03T12:00:00Z"}
///     ],
///     "start_time": "2025-12-03T10:00:00Z",
///     "end_time": "2025-12-03T14:00:00Z",
///     "duration_secs": 14400,
///     "output_count": 3,
///     "total_size_bytes": 5368709120,
///     "danmu_count": 15000
/// }
/// ```
///
/// # Errors
///
/// - `404 Not Found` - Session with the specified ID does not exist
///
#[utoipa::path(
    get,
    path = "/api/sessions/{id}",
    tag = "sessions",
    params(("id" = String, Path, description = "Session ID")),
    responses(
        (status = 200, description = "Session details", body = SessionResponse),
        (status = 404, description = "Session not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionResponse>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    let streamer_repository = state
        .streamer_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Get session by ID
    let session = session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    // Get output count
    let output_count = session_repository.get_output_count(&id).await.unwrap_or(0);

    let start_time = crate::database::time::ms_to_datetime(session.start_time);
    let end_time = session.end_time.map(crate::database::time::ms_to_datetime);

    // Calculate duration
    let duration_secs = end_time.map(|end| (end - start_time).num_seconds() as u64);

    // Parse titles JSON
    let (titles, title) = parse_titles(&session.titles);

    // Get streamer details
    let streamer = streamer_repository
        .get_streamer(&session.streamer_id)
        .await
        .ok();
    let (streamer_name, streamer_avatar) = if let Some(s) = streamer {
        (s.name, s.avatar)
    } else {
        (String::new(), None)
    };

    // Fetch danmu stats by session id (danmu_statistics.session_id).
    let danmu_count = session_repository
        .get_danmu_statistics(&session.id)
        .await
        .ok()
        .flatten()
        .map(|stats| stats.total_danmus as u64);

    // Get thumbnail URL
    let thumbnail_url = get_thumbnail_url(&session.id, session_repository.as_ref()).await;

    let response = SessionResponse {
        id: session.id.clone(),
        streamer_id: session.streamer_id,
        streamer_name,
        title,
        titles,
        start_time,
        end_time,
        duration_secs,
        output_count,
        total_size_bytes: session.total_size_bytes as u64,
        danmu_count,
        thumbnail_url,
        streamer_avatar,
    };

    Ok(Json(response))
}

/// Get full danmu statistics for a session by ID.
#[utoipa::path(
    get,
    path = "/api/sessions/{id}/danmu-statistics",
    tag = "sessions",
    params(("id" = String, Path, description = "Session ID")),
    responses(
        (status = 200, description = "Session danmu statistics", body = SessionDanmuStatisticsResponse),
        (status = 404, description = "Session or danmu statistics not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_session_danmu_statistics(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionDanmuStatisticsResponse>> {
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    // Ensure session exists so missing stats can cleanly map to 404.
    let session = session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    let stats = session_repository
        .get_danmu_statistics(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::not_found(format!("DanmuStatistics with id '{}' not found", id))
        })?;

    let danmu_rate_timeseries = stats
        .danmu_rate_timeseries
        .as_deref()
        .map(serde_json::from_str::<Vec<DanmuRateEntry>>)
        .transpose()
        .map_err(|e| ApiError::internal(format!("Failed to parse danmu rate timeseries: {e}")))?
        .unwrap_or_default()
        .into_iter()
        .map(|point| DanmuRatePoint {
            ts: point.ts,
            count: point.count,
        })
        .collect();

    let top_talkers = stats
        .top_talkers
        .as_deref()
        .map(serde_json::from_str::<Vec<TopTalkerEntry>>)
        .transpose()
        .map_err(|e| ApiError::internal(format!("Failed to parse top talkers: {e}")))?
        .unwrap_or_default()
        .into_iter()
        .map(|entry| DanmuTopTalker {
            user_id: entry.user_id,
            username: entry.username,
            message_count: entry.message_count,
        })
        .collect();

    let mut word_frequency = stats
        .word_frequency
        .as_deref()
        .map(serde_json::from_str::<Vec<DanmuWordFrequency>>)
        .transpose()
        .map_err(|e| ApiError::internal(format!("Failed to parse word frequency: {e}")))?
        .unwrap_or_default();
    word_frequency.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));

    let response = SessionDanmuStatisticsResponse {
        session_id: session.id,
        total_danmus: stats.total_danmus as u64,
        danmu_rate_timeseries,
        top_talkers,
        word_frequency,
    };

    Ok(Json(response))
}

/// Helper to get the thumbnail URL for a session
async fn get_thumbnail_url(
    session_id: &str,
    repo: &dyn crate::database::repositories::session::SessionRepository,
) -> Option<String> {
    use crate::database::models::MediaFileType;
    // We assume the repository method returns outputs ordered by creation, taking the first thumbnail found
    // Optimally we'd have a specific query for this, but filtering in app is acceptable for now given low volume per session
    let outputs = repo.get_media_outputs_for_session(session_id).await.ok()?;
    outputs
        .into_iter()
        .find(|o| o.file_type == MediaFileType::Thumbnail.as_str())
        .map(|o| format!("/api/media/{}/content", o.id))
}

/// Parse titles JSON and extract and current title.
fn parse_titles(titles_json: &Option<String>) -> (Vec<TitleChange>, String) {
    let titles_json = match titles_json {
        Some(json) => json,
        None => return (Vec::new(), String::new()),
    };

    let title_entries: Vec<TitleEntry> = serde_json::from_str(titles_json).unwrap_or_default();

    let titles: Vec<TitleChange> = title_entries
        .iter()
        .map(|entry| TitleChange {
            title: entry.title.clone(),
            timestamp: crate::database::time::ms_to_datetime(entry.ts),
        })
        .collect();

    // Get the most recent title as the current title
    let title = titles.last().map(|t| t.title.clone()).unwrap_or_default();

    (titles, title)
}

/// Delete a session by ID.
///
/// # Endpoint
///
/// `DELETE /api/sessions/:id`
///
/// # Path Parameters
///
/// - `id` - The session ID (UUID)
///
/// # Response
///
/// Returns 200 OK on success.
///
/// # Errors
///
/// - `404 Not Found` - Session with the specified ID does not exist
/// - `500 Internal Server Error` - Database error
#[utoipa::path(
    delete,
    path = "/api/sessions/{id}",
    tag = "sessions",
    params(("id" = String, Path, description = "Session ID")),
    responses(
        (status = 200, description = "Session deleted"),
        (status = 404, description = "Session not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    // Check if session exists
    let _ = session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    // Delete session
    // Note: ON DELETE CASCADE on DB tables should handle media_outputs
    // danmu_statistics might need manual deletion if not set to CASCADE, but let's assume it handles or we catch error
    session_repository
        .delete_session(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(())
}

/// Request body for batch session deletion.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct BatchDeleteRequest {
    /// List of session IDs to delete
    pub ids: Vec<String>,
}

/// Response for batch session deletion.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchDeleteResponse {
    /// Number of sessions deleted
    pub deleted: u64,
}

/// Delete multiple sessions by IDs.
///
/// # Endpoint
///
/// `POST /api/sessions/batch-delete`
///
/// # Request Body
///
/// ```json
/// {
///     "ids": ["session-id-1", "session-id-2", "session-id-3"]
/// }
/// ```
///
/// # Response
///
/// Returns the count of deleted sessions.
///
/// ```json
/// {
///     "deleted": 3
/// }
/// ```
///
/// # Errors
///
/// - `500 Internal Server Error` - Database error
#[utoipa::path(
    post,
    path = "/api/sessions/batch-delete",
    tag = "sessions",
    request_body = BatchDeleteRequest,
    responses(
        (status = 200, description = "Sessions deleted", body = BatchDeleteResponse),
        (status = 500, description = "Server error", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_sessions_batch(
    State(state): State<AppState>,
    Json(request): Json<BatchDeleteRequest>,
) -> ApiResult<Json<BatchDeleteResponse>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    // Delete sessions in batch
    let deleted = session_repository
        .delete_sessions_batch(&request.ids)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(BatchDeleteResponse { deleted }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_filter_params_default() {
        let params = SessionFilterParams::default();
        assert!(params.streamer_id.is_none());
        assert!(params.from_date.is_none());
        assert!(params.to_date.is_none());
        assert!(params.active_only.is_none());
    }

    #[test]
    fn test_parse_titles_empty() {
        let (titles, title) = parse_titles(&None);
        assert!(titles.is_empty());
        assert!(title.is_empty());
    }

    #[test]
    fn test_parse_titles_with_entries() {
        let json = r#"[
            {"ts": 1735725600000, "title": "First Stream"},
            {"ts": 1735732800000, "title": "Updated Title"}
        ]"#;

        let (titles, title) = parse_titles(&Some(json.to_string()));
        assert_eq!(titles.len(), 2);
        assert_eq!(title, "Updated Title");
    }
}
