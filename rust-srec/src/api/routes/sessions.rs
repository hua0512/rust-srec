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
    routing::get,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    PaginatedResponse, PaginationParams, SessionFilterParams, SessionResponse, TitleChange,
};
use crate::api::server::AppState;
use crate::database::models::{Pagination, SessionFilters, TitleEntry};

/// Create the sessions router.
///
/// # Routes
///
/// - `GET /` - List sessions with filtering and pagination
/// - `GET /:id` - Get a single session by ID
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/{id}", get(get_session))
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
///             "title": "Stream Title",
///             "start_time": "2025-12-03T10:00:00Z",
///             "end_time": "2025-12-03T14:00:00Z",
///             "duration_secs": 14400,
///             "output_count": 3,
///             "total_size_bytes": 5368709120
///         }
///     ],
///     "total": 50,
///     "limit": 20,
///     "offset": 0
/// }
/// ```
///
/// # Requirements
///
/// - 4.1: Return sessions matching filter criteria with pagination
/// - 4.3: Filter by streamer_id
/// - 4.4: Filter by date range
/// - 4.5: Filter for active sessions (no end_time)
async fn list_sessions(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<SessionFilterParams>,
) -> ApiResult<Json<PaginatedResponse<SessionResponse>>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    // Convert API filter params to database filter types
    let db_filters = SessionFilters {
        streamer_id: filters.streamer_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        active_only: filters.active_only,
    };

    let db_pagination = Pagination::new(pagination.limit, pagination.offset);

    // Call SessionRepository.list_sessions_filtered
    let (sessions, total) = session_repository
        .list_sessions_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    // Convert sessions to API response format
    let mut session_responses: Vec<SessionResponse> = Vec::with_capacity(sessions.len());

    for session in &sessions {
        // Get output count for each session
        let output_count = session_repository
            .get_output_count(&session.id)
            .await
            .unwrap_or(0);

        // Parse start_time
        let start_time = chrono::DateTime::parse_from_rfc3339(&session.start_time)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        // Parse end_time
        let end_time = session.end_time.as_ref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()
        });

        // Calculate duration
        let duration_secs = end_time.map(|end| (end - start_time).num_seconds() as u64);

        // Parse titles JSON
        let (titles, streamer_name, title) = parse_titles(&session.titles);

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
            total_size_bytes: 0, // Would need to sum from outputs
            danmu_count: None,   // Would need to get from danmu_statistics
        });
    }

    let response = PaginatedResponse::new(session_responses, total, pagination.limit, pagination.offset);
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
/// # Requirements
///
/// - 4.2: Return session with metadata and output count
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionResponse>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?;

    // Get session by ID
    let session = session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    // Get output count
    let output_count = session_repository
        .get_output_count(&id)
        .await
        .unwrap_or(0);

    // Parse start_time
    let start_time = chrono::DateTime::parse_from_rfc3339(&session.start_time)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    // Parse end_time
    let end_time = session.end_time.as_ref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok()
    });

    // Calculate duration
    let duration_secs = end_time.map(|end| (end - start_time).num_seconds() as u64);

    // Parse titles JSON
    let (titles, streamer_name, title) = parse_titles(&session.titles);

    // Get danmu statistics if available
    let danmu_count = if let Some(danmu_stats_id) = &session.danmu_statistics_id {
        session_repository
            .get_danmu_statistics(danmu_stats_id)
            .await
            .ok()
            .flatten()
            .map(|stats| stats.total_danmus as u64)
    } else {
        None
    };

    let response = SessionResponse {
        id: session.id.clone(),
        streamer_id: session.streamer_id.clone(),
        streamer_name,
        title,
        titles,
        start_time,
        end_time,
        duration_secs,
        output_count,
        total_size_bytes: 0, // Would need to sum from outputs
        danmu_count,
    };

    Ok(Json(response))
}

/// Parse titles JSON and extract streamer_name and current title.
fn parse_titles(titles_json: &Option<String>) -> (Vec<TitleChange>, String, String) {
    let titles_json = match titles_json {
        Some(json) => json,
        None => return (Vec::new(), String::new(), String::new()),
    };

    let title_entries: Vec<TitleEntry> = serde_json::from_str(titles_json).unwrap_or_default();

    let titles: Vec<TitleChange> = title_entries
        .iter()
        .filter_map(|entry| {
            let timestamp = chrono::DateTime::parse_from_rfc3339(&entry.ts)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()?;
            Some(TitleChange {
                title: entry.title.clone(),
                timestamp,
            })
        })
        .collect();

    // Get the most recent title as the current title
    let title = titles.last().map(|t| t.title.clone()).unwrap_or_default();

    // Streamer name is not stored in titles, would need to join with streamers table
    let streamer_name = String::new();

    (titles, streamer_name, title)
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
        let (titles, streamer_name, title) = parse_titles(&None);
        assert!(titles.is_empty());
        assert!(streamer_name.is_empty());
        assert!(title.is_empty());
    }

    #[test]
    fn test_parse_titles_with_entries() {
        let json = r#"[
            {"ts": "2025-01-01T10:00:00Z", "title": "First Stream"},
            {"ts": "2025-01-01T12:00:00Z", "title": "Updated Title"}
        ]"#;

        let (titles, _streamer_name, title) = parse_titles(&Some(json.to_string()));
        assert_eq!(titles.len(), 2);
        assert_eq!(title, "Updated Title");
    }
}
