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
    extract::{FromRef, Path, Query, State},
    routing::{get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    DanmuGiftTally, DanmuRatePoint, DanmuTopTalker, DanmuWordFrequency, PageResponse,
    PaginatedResponse, PaginationParams, SessionDanmuStatisticsResponse, SessionEventResponse,
    SessionFilterParams, SessionResponse, SessionSegmentResponse, TitleChange,
};
use crate::api::server::AppState;
use crate::database::models::{
    DanmuRateEntry, GiftTallyEntry, MediaFileType, Pagination, SessionFilters, TitleEntry,
    TopTalkerEntry, WordFrequencyEntry,
};
use crate::session::SessionEvent;

#[derive(Clone)]
pub struct SessionRouteState {
    session_repository: std::sync::Arc<dyn crate::database::repositories::SessionRepository>,
    session_event_repository:
        std::sync::Arc<dyn crate::database::repositories::SessionEventRepository>,
    streamer_repository: std::sync::Arc<dyn crate::database::repositories::StreamerRepository>,
}

impl FromRef<AppState> for SessionRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            session_repository: state.session_repository.clone(),
            session_event_repository: state.session_event_repository.clone(),
            streamer_repository: state.streamer_repository.clone(),
        }
    }
}

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
        .route("/{id}/segments", get(list_session_segments))
        .route("/{id}", get(get_session).delete(delete_session))
}

#[utoipa::path(
    get,
    path = "/api/sessions/{id}/segments",
    tag = "sessions",
    params(
        ("id" = String, Path, description = "Session ID"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of session segments with start, completion, and persistence timestamps", body = PageResponse<SessionSegmentResponse>),
        (status = 404, description = "Session not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_session_segments(
    State(state): State<SessionRouteState>,
    Path(id): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PageResponse<SessionSegmentResponse>>> {
    let session_repository = state.session_repository.clone();

    session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);
    let segments = session_repository
        .list_session_segments_page(&id, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let items = segments
        .into_iter()
        .map(|s| SessionSegmentResponse {
            id: s.id,
            session_id: s.session_id,
            segment_index: if s.segment_index < 0 {
                0
            } else {
                u32::try_from(s.segment_index).unwrap_or(u32::MAX)
            },
            file_path: s.file_path,
            duration_secs: s.duration_secs,
            size_bytes: if s.size_bytes < 0 {
                0
            } else {
                u64::try_from(s.size_bytes).unwrap_or(u64::MAX)
            },
            split_reason_code: s.split_reason_code.clone(),
            split_reason_details: s
                .split_reason_details_json
                .as_ref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()),
            created_at: s.created_at.map(crate::database::time::ms_to_datetime),
            completed_at: s.completed_at.map(crate::database::time::ms_to_datetime),
            persisted_at: crate::database::time::ms_to_datetime(s.persisted_at),
        })
        .collect();

    Ok(Json(PageResponse::new(
        items,
        effective_limit,
        pagination.offset,
    )))
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
    State(state): State<SessionRouteState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<SessionFilterParams>,
) -> ApiResult<Json<PaginatedResponse<SessionResponse>>> {
    // Get session repository from state
    let session_repository = &state.session_repository;

    let streamer_repository = &state.streamer_repository;

    // Convert API filter params to database filter types
    let db_filters = SessionFilters {
        streamer_id: filters.streamer_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        active_only: filters.active_only,
        search: filters.search,
        include_empty: filters.include_empty,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    // Call SessionRepository.list_sessions_filtered
    let (sessions, total) = session_repository
        .list_sessions_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let session_ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();
    // Sessions whose streamer was deleted carry no id to look up; their label
    // comes from the denormalized `streamer_name` below.
    let streamer_ids: Vec<String> = sessions
        .iter()
        .filter_map(|session| session.streamer_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Counts only: this list renders a number per session, and the full
    // statistics rows carry aggregate JSON blobs tens of kilobytes each.
    let (streamers, outputs, danmu_totals) = tokio::try_join!(
        streamer_repository.get_streamers_by_ids(&streamer_ids),
        session_repository.get_media_outputs_for_sessions(&session_ids),
        session_repository.get_danmu_counts_for_sessions(&session_ids),
    )
    .map_err(ApiError::from)?;

    let streamer_map: std::collections::HashMap<_, _> =
        streamers.into_iter().map(|s| (s.id.clone(), s)).collect();
    let mut output_counts = std::collections::HashMap::new();
    let mut thumbnail_urls = std::collections::HashMap::new();
    for output in outputs {
        let count = output_counts
            .entry(output.session_id.clone())
            .or_insert(0_u32);
        *count = count.saturating_add(1);
        if output.file_type == MediaFileType::Thumbnail.as_str() {
            thumbnail_urls
                .entry(output.session_id)
                .or_insert_with(|| format!("/api/media/{}/content", output.id));
        }
    }
    let danmu_counts: std::collections::HashMap<_, _> = danmu_totals
        .into_iter()
        .map(|(session_id, total)| (session_id, u64::try_from(total).unwrap_or(0)))
        .collect();

    let session_responses = sessions
        .iter()
        .map(|session| {
            let start_time = crate::database::time::ms_to_datetime(session.start_time);
            let end_time = session.end_time.map(crate::database::time::ms_to_datetime);

            let duration_secs =
                end_time.map(|end| u64::try_from((end - start_time).num_seconds()).unwrap_or(0));

            // Parse titles JSON
            let (titles, title) = parse_titles(&session.titles);

            // Prefer the live streamer row so a rename shows up immediately;
            // fall back to the name the session was recorded under once the
            // streamer is gone.
            let (streamer_name, streamer_avatar) = match session
                .streamer_id
                .as_ref()
                .and_then(|id| streamer_map.get(id))
            {
                Some(s) => (s.name.clone(), s.avatar.clone()),
                None => (session.streamer_name.clone().unwrap_or_default(), None),
            };

            SessionResponse {
                id: session.id.clone(),
                streamer_id: session.streamer_id.clone(),
                streamer_name,
                title,
                titles,
                // Lifecycle audit log isn't loaded on the list endpoint — N+1
                // queries on a paginated response. Frontend lists don't render
                // it; the detail endpoint populates it.
                events: Vec::new(),
                start_time,
                end_time,
                is_live: end_time.is_none(),
                duration_secs,
                output_count: output_counts.get(&session.id).copied().unwrap_or(0),
                total_size_bytes: u64::try_from(session.total_size_bytes).unwrap_or(0),
                danmu_count: danmu_counts.get(&session.id).copied(),
                thumbnail_url: thumbnail_urls.get(&session.id).cloned(),
                streamer_avatar,
            }
        })
        .collect();

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
    State(state): State<SessionRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionResponse>> {
    // Get session repository from state
    let session_repository = &state.session_repository;

    let streamer_repository = &state.streamer_repository;

    // Get session by ID
    let session = session_repository
        .get_session(&id)
        .await
        .map_err(ApiError::from)?;

    // Get output count
    let output_count = match session_repository.get_output_count(&id).await {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "Failed to load session output count");
            0
        }
    };

    let start_time = crate::database::time::ms_to_datetime(session.start_time);
    let end_time = session.end_time.map(crate::database::time::ms_to_datetime);

    // Calculate duration
    let duration_secs =
        end_time.map(|end| u64::try_from((end - start_time).num_seconds()).unwrap_or(0));

    // Parse titles JSON
    let (titles, title) = parse_titles(&session.titles);

    // Prefer the live streamer row so a rename shows up immediately; fall back
    // to the name the session was recorded under once the streamer is gone.
    let streamer = match session.streamer_id.as_deref() {
        Some(streamer_id) => match streamer_repository.get_streamer(streamer_id).await {
            Ok(streamer) => Some(streamer),
            Err(error) => {
                tracing::warn!(
                    streamer_id = %streamer_id,
                    %error,
                    "Failed to load streamer for session response"
                );
                None
            }
        },
        None => None,
    };
    let (streamer_name, streamer_avatar) = if let Some(s) = streamer {
        (s.name, s.avatar)
    } else {
        (session.streamer_name.clone().unwrap_or_default(), None)
    };

    // Count only; the full statistics are served by the dedicated
    // danmu-statistics endpoint rather than inflating every session response.
    let danmu_count = match session_repository
        .get_danmu_counts_for_sessions(std::slice::from_ref(&session.id))
        .await
    {
        Ok(counts) => counts
            .first()
            .map(|(_, total)| u64::try_from(*total).unwrap_or(0)),
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "Failed to load session danmu count");
            None
        }
    };

    // Get thumbnail URL
    let thumbnail_url = get_thumbnail_url(&session.id, session_repository.as_ref()).await;

    // A timeline query is an enhancement to the session response, so a
    // storage failure still degrades to an empty timeline.
    let events = match state.session_event_repository.list_for_session(&id).await {
        Ok(rows) => map_session_events(rows),
        Err(e) => {
            tracing::warn!(session_id = %id, error = %e,
                "Failed to load session events; returning empty timeline");
            Vec::new()
        }
    };

    let response = SessionResponse {
        id: session.id.clone(),
        streamer_id: session.streamer_id,
        streamer_name,
        title,
        titles,
        events,
        start_time,
        end_time,
        is_live: end_time.is_none(),
        duration_secs,
        output_count,
        total_size_bytes: u64::try_from(session.total_size_bytes).unwrap_or(0),
        danmu_count,
        thumbnail_url,
        streamer_avatar,
    };

    Ok(Json(response))
}

/// Map persisted session events to the wire-format `SessionEventResponse`.
/// Malformed stored payloads are already represented as `payload: None` by
/// the repository's domain mapping, so operators still see "something
/// happened here" with the kind discriminator.
fn map_session_events(events: Vec<SessionEvent>) -> Vec<SessionEventResponse> {
    events
        .into_iter()
        .map(|event| SessionEventResponse {
            kind: event.kind,
            occurred_at: event.occurred_at,
            payload: event.payload,
        })
        .collect()
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
    State(state): State<SessionRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionDanmuStatisticsResponse>> {
    let session_repository = &state.session_repository;

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

    let parse_talkers = |json: Option<&str>, field: &'static str| {
        json.map(serde_json::from_str::<Vec<TopTalkerEntry>>)
            .transpose()
            .map_err(|e| ApiError::internal(format!("Failed to parse {field}: {e}")))
            .map(|entries| {
                entries
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| DanmuTopTalker {
                        user_id: entry.user_id,
                        username: entry.username,
                        message_count: entry.message_count,
                        error: entry.error,
                    })
                    .collect::<Vec<_>>()
            })
    };
    let top_talkers = parse_talkers(stats.top_talkers.as_deref(), "top talkers")?;
    let top_gifters = parse_talkers(stats.top_gifters.as_deref(), "top gifters")?;

    let top_gifts = stats
        .top_gifts
        .as_deref()
        .map(serde_json::from_str::<Vec<GiftTallyEntry>>)
        .transpose()
        .map_err(|e| ApiError::internal(format!("Failed to parse top gifts: {e}")))?
        .unwrap_or_default()
        .into_iter()
        .map(|entry| DanmuGiftTally {
            name: entry.name,
            count: entry.count,
        })
        .collect();

    // Parsed as the stored entry type rather than the response type: rows written
    // before `error` existed omit the field, and only the stored type defaults it.
    let mut word_frequency: Vec<DanmuWordFrequency> = stats
        .word_frequency
        .as_deref()
        .map(serde_json::from_str::<Vec<WordFrequencyEntry>>)
        .transpose()
        .map_err(|e| ApiError::internal(format!("Failed to parse word frequency: {e}")))?
        .unwrap_or_default()
        .into_iter()
        .map(|entry| DanmuWordFrequency {
            word: entry.word,
            count: entry.count,
            error: entry.error,
        })
        .collect();
    word_frequency.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));

    let to_u64 = |value: i64| u64::try_from(value).unwrap_or(0);
    let response = SessionDanmuStatisticsResponse {
        session_id: session.id,
        total_danmus: to_u64(stats.total_danmus),
        unique_talkers: stats.unique_talkers.map(to_u64),
        chat_count: stats.chat_count.map(to_u64),
        gift_count: stats.gift_count.map(to_u64),
        duration_secs: stats.duration_secs.map(to_u64),
        start_time: stats.start_time.map(crate::database::time::ms_to_datetime),
        end_time: stats.end_time.map(crate::database::time::ms_to_datetime),
        rate_bucket_secs: stats.rate_bucket_secs.map(to_u64),
        danmu_rate_timeseries,
        top_talkers,
        top_gifters,
        top_gifts,
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
    State(state): State<SessionRouteState>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    // Get session repository from state
    let session_repository = &state.session_repository;

    // Check if session exists
    session_repository
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
    State(state): State<SessionRouteState>,
    Json(request): Json<BatchDeleteRequest>,
) -> ApiResult<Json<BatchDeleteResponse>> {
    // Get session repository from state
    let session_repository = &state.session_repository;

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
    use crate::api::models::SessionSegmentResponse;
    use chrono::{TimeZone, Utc};

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

    #[test]
    fn test_session_segment_response_serializes_lifecycle_fields() {
        let response = SessionSegmentResponse {
            id: "seg-1".to_string(),
            session_id: "sess-1".to_string(),
            segment_index: 2,
            file_path: "/tmp/seg-2.ts".to_string(),
            duration_secs: 8.5,
            size_bytes: 4096,
            split_reason_code: None,
            split_reason_details: None,
            created_at: Some(
                Utc.timestamp_millis_opt(1_700_000_000_000)
                    .single()
                    .unwrap(),
            ),
            completed_at: Some(
                Utc.timestamp_millis_opt(1_700_000_008_500)
                    .single()
                    .unwrap(),
            ),
            persisted_at: Utc
                .timestamp_millis_opt(1_700_000_010_000)
                .single()
                .unwrap(),
        };

        let value = serde_json::to_value(response).unwrap();
        assert!(value.get("created_at").is_some());
        assert!(value.get("completed_at").is_some());
        assert!(value.get("persisted_at").is_some());
    }
}
