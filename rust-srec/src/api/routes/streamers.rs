//! Streamer management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, patch, post, put},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateStreamerRequest, ExtractMetadataRequest, ExtractMetadataResponse, PaginatedResponse,
    PaginationParams, PlatformConfigResponse, StreamerFilterParams, StreamerResponse,
    UpdatePriorityRequest, UpdateStreamerRequest,
};
use crate::api::server::AppState;
use crate::domain::streamer::StreamerState;
use crate::streamer::StreamerMetadata;
use crate::utils::json::{self, JsonContext};

/// Create the streamers router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_streamer))
        .route("/", get(list_streamers))
        .route("/{id}", get(get_streamer))
        .route("/{id}", put(update_streamer))
        .route("/{id}", delete(delete_streamer))
        .route("/{id}/clear-error", post(clear_error))
        .route("/{id}/priority", patch(update_priority))
        .route("/extract-metadata", post(extract_metadata))
}

/// Convert StreamerMetadata to StreamerResponse.
fn metadata_to_response(metadata: &StreamerMetadata) -> StreamerResponse {
    StreamerResponse {
        id: metadata.id.clone(),
        name: metadata.name.clone(),
        url: metadata.url.clone(),
        platform_config_id: metadata.platform_config_id.clone(),
        template_id: metadata.template_config_id.clone(),
        state: metadata.state,
        priority: metadata.priority,
        enabled: metadata.state != StreamerState::Disabled,
        consecutive_error_count: metadata.consecutive_error_count,
        disabled_until: metadata.disabled_until,
        last_error: metadata.last_error.clone(),
        avatar_url: metadata.avatar_url.clone(),
        last_live_time: metadata.last_live_time,
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        streamer_specific_config: json::parse_optional_value_non_null(
            metadata.streamer_specific_config.as_deref(),
            JsonContext::StreamerField {
                streamer_id: &metadata.id,
                field: "streamer_specific_config",
            },
            "Invalid JSON field; omitting from response",
        ),
    }
}

#[utoipa::path(
    post,
    path = "/api/streamers",
    tag = "streamers",
    request_body = CreateStreamerRequest,
    responses(
        (status = 201, description = "Streamer created", body = StreamerResponse),
        (status = 409, description = "Streamer URL already exists", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "Validation error", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_streamer(
    State(state): State<AppState>,
    Json(request): Json<CreateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Validate URL format
    if request.url.is_empty() {
        return Err(ApiError::validation("URL cannot be empty"));
    }

    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check URL uniqueness (case-insensitive)
    if streamer_manager.url_exists(&request.url) {
        return Err(ApiError::conflict(
            "A streamer with this URL already exists",
        ));
    }

    // Generate a new ID for the streamer
    let id = uuid::Uuid::new_v4().to_string();

    // Create metadata from request
    let metadata = StreamerMetadata {
        id: id.clone(),
        name: request.name.clone(),
        url: request.url.clone(),
        platform_config_id: request.platform_config_id.clone(),
        template_config_id: request.template_id.clone(),
        state: if request.enabled {
            StreamerState::NotLive
        } else {
            StreamerState::Disabled
        },
        priority: request.priority,
        consecutive_error_count: 0,
        disabled_until: None,
        last_error: None,
        avatar_url: None,
        last_live_time: None,
        streamer_specific_config: request.streamer_specific_config.as_ref().and_then(|v| {
            if v.is_null() {
                None
            } else {
                Some(v.to_string())
            }
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Create streamer using manager
    streamer_manager
        .create_streamer(metadata.clone())
        .await
        .map_err(ApiError::from)?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[utoipa::path(
    get,
    path = "/api/streamers",
    tag = "streamers",
    params(PaginationParams, StreamerFilterParams),
    responses(
        (status = 200, description = "List of streamers", body = PaginatedResponse<StreamerResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_streamers(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<StreamerFilterParams>,
) -> ApiResult<Json<PaginatedResponse<StreamerResponse>>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Get all streamers from manager
    let mut streamers = streamer_manager.get_all();

    // Apply filters
    if let Some(platform) = &filters.platform {
        streamers.retain(|s| &s.platform_config_id == platform);
    }
    if let Some(state_str) = &filters.state
        && !state_str.is_empty()
    {
        let states: Vec<StreamerState> = state_str
            .split(',')
            .filter_map(|s| StreamerState::parse(&s.trim().to_uppercase()))
            .collect();
        streamers.retain(|s| states.contains(&s.state));
    }
    if let Some(priority) = &filters.priority {
        streamers.retain(|s| s.priority == *priority);
    }
    if let Some(enabled) = filters.enabled {
        streamers.retain(|s| (s.state != StreamerState::Disabled) == enabled);
    }
    if let Some(search) = &filters.search
        && !search.is_empty()
    {
        let search = search.to_lowercase();
        streamers.retain(|s| {
            s.name.to_lowercase().contains(&search) || s.url.to_lowercase().contains(&search)
        });
    }

    // Sort for stable pagination
    let sort_by = filters.sort_by.as_deref();
    let desc = filters
        .sort_dir
        .as_deref()
        .is_some_and(|dir| dir.eq_ignore_ascii_case("desc"));

    match sort_by {
        Some("name") => {
            streamers.sort_by(|a, b| {
                let ordering = if desc {
                    b.name.cmp(&a.name)
                } else {
                    a.name.cmp(&b.name)
                };
                ordering.then_with(|| a.id.cmp(&b.id))
            });
        }
        Some("priority") => {
            streamers.sort_by(|a, b| {
                let ordering = if desc {
                    b.priority.cmp(&a.priority)
                } else {
                    a.priority.cmp(&b.priority)
                };
                ordering
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        Some("state") => {
            streamers.sort_by(|a, b| {
                let a_state = a.state.as_str();
                let b_state = b.state.as_str();
                let ordering = if desc {
                    b_state.cmp(a_state)
                } else {
                    a_state.cmp(b_state)
                };
                ordering
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        _ => {
            // Default: LIVE streamers first, then by priority desc, name asc, id asc.
            // This ensures active streamers are always visible at the top.
            streamers.sort_by(|a, b| {
                // State priority: Active states first, then offline, then errors, then disabled
                let state_order = |s: &StreamerState| -> u8 {
                    match s {
                        StreamerState::Live => 0,
                        StreamerState::Error => 1,
                        StreamerState::FatalError => 2,
                        StreamerState::OutOfSpace => 3,
                        StreamerState::NotFound => 4,
                        StreamerState::TemporalDisabled => 5,
                        StreamerState::InspectingLive => 6,
                        StreamerState::OutOfSchedule => 7,
                        StreamerState::Cancelled => 8,
                        StreamerState::Disabled => 9,
                        StreamerState::NotLive => 10,
                    }
                };
                state_order(&a.state)
                    .cmp(&state_order(&b.state))
                    .then_with(|| b.priority.cmp(&a.priority))
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }

    // Calculate total before pagination
    let total = streamers.len() as u64;

    // Apply pagination
    let offset = pagination.offset as usize;
    let effective_limit = pagination.limit.min(100);
    let limit = effective_limit as usize;
    let streamers: Vec<_> = streamers
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| {
            // tracing::debug!("Streamer {} state: {:?}", s.name, s.state);
            metadata_to_response(&s)
        })
        .collect();

    let response = PaginatedResponse::new(streamers, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/api/streamers/{id}",
    tag = "streamers",
    params(("id" = String, Path, description = "Streamer ID")),
    responses(
        (status = 200, description = "Streamer details", body = StreamerResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Get streamer by ID
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::not_found(format!("Streamer with id '{}' not found", id)))?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[utoipa::path(
    put,
    path = "/api/streamers/{id}",
    tag = "streamers",
    params(("id" = String, Path, description = "Streamer ID")),
    request_body = UpdateStreamerRequest,
    responses(
        (status = 200, description = "Streamer updated", body = StreamerResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "URL already exists", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check URL uniqueness if URL is being changed (case-insensitive)
    if let Some(ref new_url) = request.url
        && streamer_manager.url_exists_for_other(new_url, &id)
    {
        return Err(ApiError::conflict(
            "A streamer with this URL already exists",
        ));
    }

    // Convert enabled flag to state if provided
    let current_metadata = streamer_manager.get_streamer(&id);
    let current_state = current_metadata.as_ref().map(|m| m.state);

    tracing::debug!(
        streamer_id = %id,
        current_state = ?current_state,
        request_enabled = ?request.enabled,
        "Processing update_streamer state transition"
    );

    let new_state = match request.enabled {
        Some(true) => {
            // User wants to enable the streamer
            // Only transition to NotLive if:
            // 1. Currently Disabled (manual disable)
            // 2. In an error state that can be recovered (FatalError, Error, TemporalDisabled, etc.)
            // Otherwise preserve current state (e.g., Live, NotLive, InspectingLive)
            match current_metadata {
                Some(metadata) => {
                    let current = metadata.state;
                    if current == StreamerState::Disabled || current.is_error() {
                        // Disabled or error state: transition to NotLive to restart monitoring
                        if current.can_transition_to(StreamerState::NotLive) {
                            tracing::debug!(
                                streamer_id = %id,
                                from = ?current,
                                to = "NotLive",
                                "Transitioning from disabled/error state"
                            );
                            Some(StreamerState::NotLive)
                        } else {
                            tracing::debug!(
                                streamer_id = %id,
                                current = ?current,
                                "Invalid transition, preserving current state"
                            );
                            None // Invalid transition, preserve current
                        }
                    } else {
                        tracing::debug!(
                            streamer_id = %id,
                            current = ?current,
                            "Active state, preserving current state"
                        );
                        None // Active state (Live, NotLive, etc.): preserve current
                    }
                }
                None => {
                    tracing::debug!(streamer_id = %id, "Streamer not found, using NotLive fallback");
                    Some(StreamerState::NotLive) // Fallback for new streamers
                }
            }
        }
        Some(false) => {
            tracing::debug!(streamer_id = %id, "Disabling streamer");
            Some(StreamerState::Disabled)
        }
        None => None,
    };

    tracing::debug!(
        streamer_id = %id,
        new_state = ?new_state,
        "Computed new state for update"
    );

    let new_priority = request.priority;

    // `template_id` supports "missing" (no update) vs explicit `null` (clear).
    let template_config_id = request.template_id;

    // Use partial_update_streamer for atomic update
    let metadata = streamer_manager
        .partial_update_streamer(crate::streamer::manager::StreamerUpdateParams {
            id: id.clone(),
            name: request.name,
            url: request.url,
            template_config_id,
            priority: new_priority,
            state: new_state,
            streamer_specific_config: request.streamer_specific_config.map(|v| {
                if v.is_null() {
                    None
                } else {
                    Some(v.to_string())
                }
            }),
        })
        .await
        .map_err(ApiError::from)?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[utoipa::path(
    delete,
    path = "/api/streamers/{id}",
    tag = "streamers",
    params(("id" = String, Path, description = "Streamer ID")),
    responses(
        (status = 200, description = "Streamer deleted", body = crate::api::openapi::MessageResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Delete the streamer
    streamer_manager
        .delete_streamer(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Streamer '{}' deleted successfully", id)
    })))
}

#[utoipa::path(
    post,
    path = "/api/streamers/{id}/clear-error",
    tag = "streamers",
    params(("id" = String, Path, description = "Streamer ID")),
    responses(
        (status = 200, description = "Error cleared", body = StreamerResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn clear_error(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Clear error state (resets consecutive_error_count and disabled_until)
    streamer_manager
        .clear_error_state(&id)
        .await
        .map_err(ApiError::from)?;

    // Get updated metadata
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::internal("Failed to retrieve streamer after clearing error"))?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[utoipa::path(
    patch,
    path = "/api/streamers/{id}/priority",
    tag = "streamers",
    params(("id" = String, Path, description = "Streamer ID")),
    request_body = UpdatePriorityRequest,
    responses(
        (status = 200, description = "Priority updated", body = StreamerResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_priority(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePriorityRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Update priority
    streamer_manager
        .update_priority(&id, request.priority)
        .await
        .map_err(ApiError::from)?;

    // Get updated metadata
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::internal("Failed to retrieve streamer after priority update"))?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Priority;

    #[test]
    fn test_create_streamer_request_validation() {
        let request = CreateStreamerRequest {
            name: "Test".to_string(),
            url: "".to_string(),
            platform_config_id: "platform1".to_string(),
            template_id: None,
            priority: Priority::Normal,
            enabled: true,
            streamer_specific_config: None,
        };

        // URL is empty, should fail validation
        assert!(request.url.is_empty());
    }

    #[test]
    fn test_metadata_to_response() {
        let metadata = StreamerMetadata {
            id: "test-id".to_string(),
            name: "Test Streamer".to_string(),
            url: "https://twitch.tv/test".to_string(),
            avatar_url: None,
            platform_config_id: "twitch".to_string(),
            template_config_id: Some("template1".to_string()),
            state: StreamerState::Live,
            priority: Priority::High,
            consecutive_error_count: 2,
            disabled_until: None,
            last_error: Some("test error".to_string()),
            last_live_time: Some(chrono::Utc::now()),
            streamer_specific_config: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let response = metadata_to_response(&metadata);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.name, "Test Streamer");
        assert_eq!(response.url, "https://twitch.tv/test");
        assert_eq!(response.platform_config_id, "twitch");
        assert_eq!(response.template_id, Some("template1".to_string()));
        assert_eq!(response.state, StreamerState::Live);
        assert!(response.enabled); // Live state means enabled
        assert_eq!(response.consecutive_error_count, 2);
        assert_eq!(response.last_error, Some("test error".to_string()));
    }

    #[test]
    fn test_disabled_state_means_not_enabled() {
        let metadata = StreamerMetadata {
            id: "test-id".to_string(),
            name: "Test".to_string(),
            url: "https://example.com".to_string(),
            avatar_url: None,
            platform_config_id: "platform".to_string(),
            template_config_id: None,
            state: StreamerState::Disabled,
            priority: Priority::Normal,
            consecutive_error_count: 0,
            disabled_until: None,
            last_error: None,
            last_live_time: None,
            streamer_specific_config: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let response = metadata_to_response(&metadata);
        assert!(!response.enabled);
    }
}

#[utoipa::path(
    post,
    path = "/api/streamers/extract-metadata",
    tag = "streamers",
    request_body = ExtractMetadataRequest,
    responses(
        (status = 200, description = "Metadata extracted", body = ExtractMetadataResponse),
        (status = 422, description = "Invalid URL", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn extract_metadata(
    State(state): State<AppState>,
    Json(request): Json<ExtractMetadataRequest>,
) -> ApiResult<Json<ExtractMetadataResponse>> {
    use crate::domain::value_objects::StreamerUrl;

    // Validate URL format
    let url = match StreamerUrl::new(&request.url) {
        Ok(u) => u,
        Err(e) => return Err(ApiError::validation(e.to_string())),
    };

    // Extract platform info
    let mut platform_name = url.platform().map(|s| s.to_string());
    let channel_id = url.channel_id();

    // Get config service
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get all platform configs
    let all_configs = config_service
        .list_platform_configs()
        .await
        .map_err(ApiError::from)?;

    // If the URL doesn't match any built-in platform regex, try to detect whether
    // the external `streamlink` CLI can handle it and suggest the `streamlink`
    // pseudo-platform.
    if platform_name.is_none() {
        let mut cmd = process_utils::tokio_command(
            std::env::var("STREAMLINK_PATH").unwrap_or_else(|_| "streamlink".to_string()),
        );
        cmd.arg("--can-handle-url-no-redirect")
            .arg(&request.url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let can_handle = cmd.status().await.is_ok_and(|s| s.success());
        if can_handle {
            platform_name = Some("streamlink".to_string());
        }
    }

    // Filter configs based on detected platform
    // If platform is detected, only return configs for that platform
    // If not detected, return all configs (user must choose)
    // We treat platform names case-insensitively
    let valid_configs: Vec<_> = all_configs
        .into_iter()
        .filter(|c| {
            if let Some(detected) = platform_name.as_deref() {
                c.platform_name.eq_ignore_ascii_case(detected)
            } else {
                true
            }
        })
        .map(|c| PlatformConfigResponse {
            id: c.id,
            name: c.platform_name,
            fetch_delay_ms: c.fetch_delay_ms.map(|v| v as u64),
            download_delay_ms: c.download_delay_ms.map(|v| v as u64),
            record_danmu: c.record_danmu,
            cookies: c.cookies,
            platform_specific_config: c.platform_specific_config,
            proxy_config: c.proxy_config,
            output_folder: c.output_folder,
            output_filename_template: c.output_filename_template,
            download_engine: c.download_engine,
            stream_selection_config: c.stream_selection_config,
            output_file_format: c.output_file_format,
            min_segment_size_bytes: c.min_segment_size_bytes.map(|v| v as u64),
            max_download_duration_secs: c.max_download_duration_secs.map(|v| v as u64),
            max_part_size_bytes: c.max_part_size_bytes.map(|v| v as u64),
            download_retry_policy: c.download_retry_policy,
            event_hooks: c.event_hooks,
            pipeline: c.pipeline,
            session_complete_pipeline: c.session_complete_pipeline,
            paired_segment_pipeline: c.paired_segment_pipeline,
        })
        .collect();

    Ok(Json(ExtractMetadataResponse {
        platform: platform_name,
        valid_platform_configs: valid_configs,
        channel_id,
    }))
}
