//! Streamer management routes.

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
    routing::{delete, get, patch, post, put},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    BatchStreamerAction, BatchStreamerItemResult, BatchStreamerRequest, BatchStreamerResponse,
    CreateStreamerRequest, ExtractMetadataRequest, ExtractMetadataResponse, PaginatedResponse,
    PaginationParams, PlatformConfigResponse, StreamerCheckHistoryEntry,
    StreamerCheckHistoryResponse, StreamerFilterParams, StreamerResponse, UpdatePriorityRequest,
    UpdateStreamerRequest,
};
use crate::api::server::AppState;
use crate::database::models::PlatformConfigDbModel;
use crate::domain::streamer::StreamerState;
use crate::domain::value_objects::StreamerUrl;
use crate::streamer::{StreamerMetadata, manager::StreamerUpdateParams};
use crate::utils::json::{self, JsonContext};

/// The `ConfigService` instantiation carried by [`StreamerRouteState`].
type StreamerConfigService = crate::config::ConfigService<
    crate::database::repositories::config::SqlxConfigRepository,
    crate::database::repositories::streamer::SqlxStreamerRepository,
>;

#[derive(Clone)]
pub struct StreamerRouteState {
    config_service: std::sync::Arc<StreamerConfigService>,
    streamer_manager: std::sync::Arc<
        crate::streamer::StreamerManager<
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
    streamer_check_history_repository:
        std::sync::Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository>,
    runtime_coordinator: std::sync::Arc<crate::services::runtime_coordinator::RuntimeCoordinator>,
}

impl FromRef<AppState> for StreamerRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
            streamer_manager: state.streamer_manager.clone(),
            streamer_check_history_repository: state.streamer_check_history_repository.clone(),
            runtime_coordinator: state.services.runtime_coordinator.clone(),
        }
    }
}

impl StreamerRouteState {
    /// Assemble the route state from a `ServiceContainer`'s services directly,
    /// for tests that drive these handlers without standing up an `AppState`
    /// (which needs a `LoggingConfig`, and with it a global tracing subscriber).
    #[cfg(test)]
    pub(crate) fn for_test(
        config_service: std::sync::Arc<StreamerConfigService>,
        streamer_manager: std::sync::Arc<
            crate::streamer::StreamerManager<
                crate::database::repositories::streamer::SqlxStreamerRepository,
            >,
        >,
        streamer_check_history_repository: std::sync::Arc<
            dyn crate::database::repositories::StreamerCheckHistoryRepository,
        >,
        runtime_coordinator: std::sync::Arc<
            crate::services::runtime_coordinator::RuntimeCoordinator,
        >,
    ) -> Self {
        Self {
            config_service,
            streamer_manager,
            streamer_check_history_repository,
            runtime_coordinator,
        }
    }
}

/// Metadata for a streamer the caller may still act on.
///
/// A row carrying `deleted_at` is gone as far as the API is concerned:
/// `RuntimeCoordinator::retire_streamer` is standing its runtime down and the
/// reaper removes the row once that finishes. `StreamerManager::get_streamer`
/// keeps returning it because the runtime decides on
/// `StreamerMetadata::is_active`, so every route has to reject it here.
fn live_streamer(
    streamer_manager: &crate::streamer::StreamerManager<
        crate::database::repositories::streamer::SqlxStreamerRepository,
    >,
    id: &str,
) -> ApiResult<StreamerMetadata> {
    streamer_manager
        .get_streamer(id)
        .filter(|metadata| !metadata.is_deleted())
        .ok_or_else(|| ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

/// Create the streamers router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_streamer))
        .route("/", get(list_streamers))
        .route("/batch", post(batch_streamers))
        .route("/{id}", get(get_streamer))
        .route("/{id}", put(update_streamer))
        .route("/{id}", delete(delete_streamer))
        .route("/{id}/clear-error", post(clear_error))
        .route("/{id}/priority", patch(update_priority))
        .route("/{id}/check-history", get(get_check_history))
        .route("/extract-metadata", post(extract_metadata))
}

const MAX_BATCH_SIZE: usize = 100;

fn validate_batch_ids(ids: &[String]) -> ApiResult<()> {
    if ids.is_empty() {
        return Err(ApiError::validation("At least one streamer ID is required"));
    }
    if ids.len() > MAX_BATCH_SIZE {
        return Err(ApiError::validation(format!(
            "A batch may contain at most {MAX_BATCH_SIZE} streamer IDs"
        )));
    }
    if ids.iter().any(|id| id.trim().is_empty()) {
        return Err(ApiError::validation("Streamer IDs cannot be empty"));
    }

    let unique_ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if unique_ids.len() != ids.len() {
        return Err(ApiError::validation(
            "Streamer IDs must be unique within a batch",
        ));
    }

    Ok(())
}

fn state_for_enabled(current: Option<StreamerState>, enabled: bool) -> Option<StreamerState> {
    if !enabled {
        return Some(StreamerState::Disabled);
    }

    match current {
        Some(state) if state == StreamerState::Disabled || state.is_error() => state
            .can_transition_to(StreamerState::NotLive)
            .then_some(StreamerState::NotLive),
        Some(_) => None,
        None => Some(StreamerState::NotLive),
    }
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

/// Pseudo-platform assigned to URLs that no built-in regex claims but the `streamlink` CLI can
/// handle. Matches the `platform-streamlink` row seeded by the initial schema migration.
const STREAMLINK_PLATFORM: &str = "streamlink";

/// A URL's platform and the `platform_config` row it belongs to.
struct ResolvedPlatform {
    /// The name as reported by [`StreamerUrl::platform`] (capitalised, e.g. `Bilibili`), or
    /// [`STREAMLINK_PLATFORM`]. Differs in case from `config.platform_name`, which is stored
    /// lowercase.
    name: String,
    config: PlatformConfigDbModel,
}

/// Ask the external `streamlink` CLI whether it can handle a URL.
///
/// A missing or failing binary counts as "no", so detection falls through to "unsupported"
/// rather than erroring.
async fn streamlink_can_handle(url: &str) -> bool {
    let mut cmd = process_utils::tokio_command(
        std::env::var("STREAMLINK_PATH").unwrap_or_else(|_| "streamlink".to_string()),
    );
    cmd.arg("--can-handle-url-no-redirect")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.status().await.is_ok_and(|s| s.success())
}

/// Resolve the platform configuration a streamer URL belongs to.
///
/// `platform_config.platform_name` is `UNIQUE`, so a URL maps to at most one row and the caller
/// never has a choice to make. Returns `Ok(None)` when the platform cannot be determined at all;
/// callers decide whether that is fatal.
///
/// `probe_streamlink` gates [`streamlink_can_handle`], the only way to classify a URL that matches
/// no built-in regex. Callers that are not changing a streamer's URL pass `false` to keep the
/// subprocess off the write path.
async fn resolve_platform_for_url(
    url: &StreamerUrl,
    config_service: &StreamerConfigService,
    probe_streamlink: bool,
) -> ApiResult<Option<ResolvedPlatform>> {
    let mut name = url.platform().map(str::to_string);

    if name.is_none() && probe_streamlink && streamlink_can_handle(url.as_str()).await {
        name = Some(STREAMLINK_PLATFORM.to_string());
    }

    let Some(name) = name else {
        return Ok(None);
    };

    let configs = config_service
        .list_platform_configs()
        .await
        .map_err(ApiError::from)?;

    let config = find_platform_config(configs, &name).ok_or_else(|| {
        ApiError::validation(format!("No platform configuration exists for '{name}'"))
    })?;

    Ok(Some(ResolvedPlatform { name, config }))
}

/// Pick the platform config for a detected platform name.
///
/// [`StreamerUrl::platform`] reports capitalised names (`Bilibili`) while `platform_name` is
/// seeded lowercase, so the comparison must ignore case. `platform_name` is `UNIQUE`, so at most
/// one row can match.
fn find_platform_config(
    configs: Vec<PlatformConfigDbModel>,
    name: &str,
) -> Option<PlatformConfigDbModel> {
    configs
        .into_iter()
        .find(|config| config.platform_name.eq_ignore_ascii_case(name))
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
    State(state): State<StreamerRouteState>,
    Json(request): Json<CreateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Validate URL format
    let url = StreamerUrl::new(&request.url).map_err(|e| ApiError::validation(e.to_string()))?;

    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    // Check URL uniqueness (case-insensitive) before resolving, so a duplicate never pays for
    // the streamlink probe.
    // A streamer marked deleted still owns its URL under the
    // `streamers.url COLLATE NOCASE UNIQUE` constraint until the reaper removes
    // the row, so say which of the two conflicts this is.
    if let Some(existing) = streamer_manager.get_streamer_by_url(&request.url) {
        return Err(if existing.is_deleted() {
            ApiError::conflict("A streamer with this URL is still being removed; try again shortly")
        } else {
            ApiError::conflict("A streamer with this URL already exists")
        });
    }

    // The platform configuration is derived from the URL; `request.platform_config_id` is ignored.
    let platform_config_id = resolve_platform_for_url(&url, &state.config_service, true)
        .await?
        .ok_or_else(|| {
            ApiError::validation("Unsupported URL: no platform recognizes this address")
        })?
        .config
        .id;

    // Generate a new ID for the streamer
    let id = uuid::Uuid::new_v4().to_string();

    // Create metadata from request
    let metadata = StreamerMetadata {
        id: id.clone(),
        name: request.name.clone(),
        url: request.url.clone(),
        platform_config_id,
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
        offline_check_count: 3,
        offline_check_delay_ms: 20_000,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
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
    State(state): State<StreamerRouteState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<StreamerFilterParams>,
) -> ApiResult<Json<PaginatedResponse<StreamerResponse>>> {
    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    // Resolve every filter once, then clone only the matching entries out of
    // the manager's map: a narrow filter (the dashboard's `state=LIVE`) must
    // not copy every streamer first.
    let platform = filters.platform.as_deref();
    let template = filters.template.as_deref();
    let template_unassigned = filters.template_unassigned == Some(true);
    let states: Option<Vec<StreamerState>> = filters
        .state
        .as_deref()
        .filter(|state_str| !state_str.is_empty())
        .map(|state_str| {
            state_str
                .split(',')
                .filter_map(|s| StreamerState::parse(&s.trim().to_uppercase()))
                .collect()
        });
    let priority = filters.priority.as_ref();
    let enabled = filters.enabled;
    let search = filters
        .search
        .as_deref()
        .filter(|search| !search.is_empty())
        .map(str::to_lowercase);

    let mut streamers = streamer_manager.get_filtered(|s| {
        platform.is_none_or(|platform| s.platform_config_id == platform)
            && template.is_none_or(|template| s.template_config_id.as_deref() == Some(template))
            && (!template_unassigned || s.template_config_id.is_none())
            && states
                .as_ref()
                .is_none_or(|states| states.contains(&s.state))
            && priority.is_none_or(|priority| s.priority == *priority)
            && enabled.is_none_or(|enabled| (s.state != StreamerState::Disabled) == enabled)
            && search.as_ref().is_none_or(|search| {
                s.name.to_lowercase().contains(search) || s.url.to_lowercase().contains(search)
            })
    });

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
        Some("updated_at") => {
            streamers.sort_by(|a, b| {
                let ordering = if desc {
                    b.updated_at.cmp(&a.updated_at)
                } else {
                    a.updated_at.cmp(&b.updated_at)
                };
                ordering.then_with(|| a.id.cmp(&b.id))
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

/// Apply one mutation to multiple streamers.
///
/// Valid requests are processed independently in request order. The response
/// reports failures per streamer so successful cache updates and lifecycle
/// events are not rolled back when another item fails.
///
/// # Errors
///
/// Returns a validation error when IDs are empty, duplicated, or exceed the
/// batch limit. Assigning an unknown template returns a not-found error before
/// any streamer is mutated.
#[utoipa::path(
    post,
    path = "/api/streamers/batch",
    tag = "streamers",
    request_body = BatchStreamerRequest,
    responses(
        (status = 200, description = "Batch mutation results", body = BatchStreamerResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "Invalid batch request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn batch_streamers(
    State(state): State<StreamerRouteState>,
    Json(request): Json<BatchStreamerRequest>,
) -> ApiResult<Json<BatchStreamerResponse>> {
    validate_batch_ids(&request.ids)?;

    let streamer_manager = &state.streamer_manager;

    if let BatchStreamerAction::SetTemplate {
        template_id: Some(template_id),
    } = &request.action
    {
        let config_service = &state.config_service;
        config_service
            .get_template_config(template_id)
            .await
            .map_err(ApiError::from)?;
    }

    let requested = request.ids.len();
    let mut results = Vec::with_capacity(requested);

    for id in request.ids {
        let result: crate::Result<()> = async {
            match &request.action {
                BatchStreamerAction::SetEnabled { enabled } => {
                    let current = streamer_manager
                        .get_streamer(&id)
                        .filter(|metadata| !metadata.is_deleted())
                        .ok_or_else(|| crate::Error::not_found("Streamer", &id))?;
                    if let Some(new_state) = state_for_enabled(Some(current.state), *enabled)
                        && new_state != current.state
                    {
                        streamer_manager
                            .partial_update_streamer(StreamerUpdateParams {
                                id: id.clone(),
                                name: None,
                                url: None,
                                platform_config_id: None,
                                template_config_id: None,
                                priority: None,
                                state: Some(new_state),
                                streamer_specific_config: None,
                            })
                            .await?;
                    }
                }
                BatchStreamerAction::SetTemplate { template_id } => {
                    streamer_manager
                        .partial_update_streamer(StreamerUpdateParams {
                            id: id.clone(),
                            name: None,
                            url: None,
                            platform_config_id: None,
                            template_config_id: Some(template_id.clone()),
                            priority: None,
                            state: None,
                            streamer_specific_config: None,
                        })
                        .await?;
                }
                BatchStreamerAction::SetPriority { priority } => {
                    streamer_manager
                        .partial_update_streamer(StreamerUpdateParams {
                            id: id.clone(),
                            name: None,
                            url: None,
                            platform_config_id: None,
                            template_config_id: None,
                            priority: Some(*priority),
                            state: None,
                            streamer_specific_config: None,
                        })
                        .await?;
                }
                BatchStreamerAction::Delete => {
                    // `OBSERVE_RETIREMENT` rather than the single-delete bounds:
                    // these run in sequence for up to `MAX_BATCH_SIZE` ids, so
                    // waiting per streamer would add up inside one request.
                    // Everything still in flight keeps its `deleted_at` marker
                    // and `ServiceContainer::spawn_streamer_reaper` removes it.
                    if !state
                        .runtime_coordinator
                        .delete_streamer(
                            &id,
                            crate::services::runtime_coordinator::OBSERVE_RETIREMENT,
                        )
                        .await?
                    {
                        return Err(crate::Error::not_found("Streamer", &id));
                    }
                }
            }
            Ok(())
        }
        .await;

        match result {
            Ok(()) => results.push(BatchStreamerItemResult {
                id,
                success: true,
                code: None,
                error: None,
            }),
            Err(error) => {
                let api_error = ApiError::from(error);
                tracing::warn!(
                    streamer_id = %id,
                    error_code = %api_error.code,
                    "Batch streamer mutation failed"
                );
                results.push(BatchStreamerItemResult {
                    id,
                    success: false,
                    code: Some(api_error.code),
                    error: Some(api_error.message),
                });
            }
        }
    }

    let succeeded = results.iter().filter(|result| result.success).count();
    Ok(Json(BatchStreamerResponse {
        requested,
        succeeded,
        failed: requested - succeeded,
        results,
    }))
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
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    // Get streamer by ID
    let metadata = live_streamer(streamer_manager, &id)?;

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
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    // Check URL uniqueness if URL is being changed (case-insensitive)
    if let Some(ref new_url) = request.url
        && streamer_manager.url_exists_for_other(new_url, &id)
    {
        return Err(ApiError::conflict(
            "A streamer with this URL already exists",
        ));
    }

    let current = live_streamer(streamer_manager, &id)?;
    let current_state = Some(current.state);

    tracing::debug!(
        streamer_id = %id,
        current_state = ?current_state,
        request_enabled = ?request.enabled,
        "Processing update_streamer state transition"
    );

    // Re-derive the platform configuration from the incoming URL. Doing this on every request that
    // carries a `url` — not just on a change — is what lets a plain save correct a streamer whose
    // stored `platform_config_id` disagrees with its URL.
    let platform_config_id = match request.url.as_deref() {
        Some(new_url) => {
            let url = StreamerUrl::new(new_url).map_err(|e| ApiError::validation(e.to_string()))?;
            let url_changed = !current.url.eq_ignore_ascii_case(new_url);

            // The streamlink probe spawns a subprocess, so only pay for it when the URL actually
            // changed; an unchanged URL that no regex claims keeps the id it already has.
            match resolve_platform_for_url(&url, &state.config_service, url_changed).await? {
                Some(resolved) => Some(resolved.config.id),
                None if url_changed => {
                    return Err(ApiError::validation(
                        "Unsupported URL: no platform recognizes this address",
                    ));
                }
                None => None,
            }
        }
        None => None,
    };

    let new_state = request
        .enabled
        .and_then(|enabled| state_for_enabled(current_state, enabled));

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
        .partial_update_streamer(StreamerUpdateParams {
            id: id.clone(),
            name: request.name,
            url: request.url,
            platform_config_id,
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
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    live_streamer(&state.streamer_manager, &id)?;
    // `false` means another caller already owns this streamer's retirement,
    // which for the client is the same outcome as this call performing it.
    let _marked = delete_streamer_through_runtime(&state, &id).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Streamer '{}' deleted successfully", id)
    })))
}

/// The one deletion path behind `DELETE /api/streamers/{id}`,
/// `BatchStreamerAction::Delete` and the MCP `streamer_delete` tool.
///
/// `RuntimeCoordinator::delete_streamer` commits the `streamers.deleted_at`
/// marker before it waits on anything, so the streamer is gone from every list
/// the moment this returns whether or not its last recording's post-processing
/// has finished; `ServiceContainer::spawn_streamer_reaper` removes the row when
/// it has. `Ok(false)` means no row was marked, which after
/// [`live_streamer`] can only be a concurrent delete of the same streamer.
async fn delete_streamer_through_runtime(state: &StreamerRouteState, id: &str) -> ApiResult<bool> {
    state
        .runtime_coordinator
        .delete_streamer(
            id,
            crate::services::runtime_coordinator::INTERACTIVE_RETIREMENT,
        )
        .await
        .map_err(ApiError::from)
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
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    live_streamer(streamer_manager, &id)?;

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
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePriorityRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = &state.streamer_manager;

    live_streamer(streamer_manager, &id)?;

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

    /// Build a platform config row carrying only the fields platform matching reads.
    fn platform_config(id: &str, platform_name: &str) -> PlatformConfigDbModel {
        PlatformConfigDbModel {
            id: id.to_string(),
            platform_name: platform_name.to_string(),
            fetch_delay_ms: None,
            download_delay_ms: None,
            cookies: None,
            platform_specific_config: None,
            proxy_config: None,
            record_danmu: None,
            danmu_statistics: None,
            output_folder: None,
            output_filename_template: None,
            download_engine: None,
            extractor: None,
            stream_selection_config: None,
            output_file_format: None,
            min_segment_size_bytes: None,
            max_download_duration_secs: None,
            max_part_size_bytes: None,
            download_retry_policy: None,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            offline_check_count: None,
            offline_check_delay_ms: None,
        }
    }

    fn seeded_configs() -> Vec<PlatformConfigDbModel> {
        vec![
            platform_config("platform-douyin", "douyin"),
            platform_config("platform-bilibili", "bilibili"),
            platform_config("platform-soop", "soop"),
            platform_config("platform-streamlink", STREAMLINK_PLATFORM),
        ]
    }

    /// `StreamerUrl::platform` reports `Bilibili` while the seeded row is `bilibili`; an
    /// exact-match lookup would miss and leave the streamer on its previous platform.
    #[test]
    fn test_find_platform_config_matches_detected_name_ignoring_case() {
        let detected = StreamerUrl::new("https://live.bilibili.com/4063253")
            .expect("valid url")
            .platform()
            .expect("bilibili is a built-in platform");
        assert_eq!(detected, "Bilibili");

        let matched = find_platform_config(seeded_configs(), detected)
            .expect("bilibili config should match case-insensitively");
        assert_eq!(matched.id, "platform-bilibili");
    }

    /// `SOOP` is fully uppercase in the detection table and lowercase in the seed data.
    #[test]
    fn test_find_platform_config_matches_uppercase_detected_name() {
        let matched =
            find_platform_config(seeded_configs(), "SOOP").expect("soop config should match");
        assert_eq!(matched.id, "platform-soop");
    }

    #[test]
    fn test_find_platform_config_returns_none_when_absent() {
        assert!(find_platform_config(seeded_configs(), "twitch").is_none());
    }

    /// A URL no built-in regex claims yields no platform, so the caller must decide whether to
    /// probe streamlink rather than silently picking a config.
    #[test]
    fn test_unrecognized_url_detects_no_platform() {
        let url = StreamerUrl::new("https://example.com/some-channel").expect("valid url");
        assert!(url.platform().is_none());
    }

    #[test]
    fn test_streamer_url_rejects_malformed_input() {
        assert!(StreamerUrl::new("not-a-url").is_err());
    }

    #[test]
    fn test_create_streamer_request_validation() {
        let request = CreateStreamerRequest {
            name: "Test".to_string(),
            url: "".to_string(),
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
            offline_check_count: 3,
            offline_check_delay_ms: 20_000,
            created_at: chrono::Utc::now(),
            deleted_at: None,
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
            offline_check_count: 3,
            offline_check_delay_ms: 20_000,
            created_at: chrono::Utc::now(),
            deleted_at: None,
            updated_at: chrono::Utc::now(),
        };

        let response = metadata_to_response(&metadata);
        assert!(!response.enabled);
    }

    #[test]
    fn test_validate_batch_ids() {
        assert!(validate_batch_ids(&["streamer-1".to_string()]).is_ok());
        assert!(validate_batch_ids(&[]).is_err());
        assert!(validate_batch_ids(&["".to_string()]).is_err());
        assert!(validate_batch_ids(&["streamer-1".to_string(), "streamer-1".to_string()]).is_err());

        let oversized = (0..=MAX_BATCH_SIZE)
            .map(|index| format!("streamer-{index}"))
            .collect::<Vec<_>>();
        assert!(validate_batch_ids(&oversized).is_err());
    }

    #[test]
    fn test_state_for_enabled_matches_single_update_semantics() {
        assert_eq!(
            state_for_enabled(Some(StreamerState::Disabled), true),
            Some(StreamerState::NotLive)
        );
        assert_eq!(
            state_for_enabled(Some(StreamerState::FatalError), true),
            Some(StreamerState::NotLive)
        );
        assert_eq!(state_for_enabled(Some(StreamerState::Live), true), None);
        assert_eq!(
            state_for_enabled(Some(StreamerState::Live), false),
            Some(StreamerState::Disabled)
        );
        assert_eq!(state_for_enabled(None, true), Some(StreamerState::NotLive));
    }

    #[test]
    fn test_batch_action_deserialization() {
        let request: BatchStreamerRequest = serde_json::from_value(serde_json::json!({
            "ids": ["streamer-1"],
            "action": {
                "type": "set_template",
                "template_id": null
            }
        }))
        .expect("batch request should deserialize");

        assert!(matches!(
            request.action,
            BatchStreamerAction::SetTemplate { template_id: None }
        ));
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
    State(state): State<StreamerRouteState>,
    Json(request): Json<ExtractMetadataRequest>,
) -> ApiResult<Json<ExtractMetadataResponse>> {
    let url = StreamerUrl::new(&request.url).map_err(|e| ApiError::validation(e.to_string()))?;
    let channel_id = url.channel_id();

    let resolved = resolve_platform_for_url(&url, &state.config_service, true).await?;

    // A resolved URL yields exactly the one config it maps to. When nothing matched, fall back to
    // listing every config so the caller can still show what exists.
    let (platform, configs) = match resolved {
        Some(ResolvedPlatform { name, config }) => (Some(name), vec![config]),
        None => (
            None,
            state
                .config_service
                .list_platform_configs()
                .await
                .map_err(ApiError::from)?,
        ),
    };

    Ok(Json(ExtractMetadataResponse {
        platform,
        valid_platform_configs: configs
            .into_iter()
            .map(platform_config_to_response)
            .collect(),
        channel_id,
    }))
}

/// Convert a platform config row to its API representation.
fn platform_config_to_response(config: PlatformConfigDbModel) -> PlatformConfigResponse {
    PlatformConfigResponse {
        id: config.id,
        name: config.platform_name,
        fetch_delay_ms: config.fetch_delay_ms.map(|v| v as u64),
        download_delay_ms: config.download_delay_ms.map(|v| v as u64),
        record_danmu: config.record_danmu,
        danmu_statistics: config.danmu_statistics,
        cookies: config.cookies,
        platform_specific_config: config.platform_specific_config,
        proxy_config: config.proxy_config,
        output_folder: config.output_folder,
        output_filename_template: config.output_filename_template,
        download_engine: config.download_engine,
        extractor: config.extractor,
        stream_selection_config: config.stream_selection_config,
        output_file_format: config.output_file_format,
        min_segment_size_bytes: config.min_segment_size_bytes.map(|v| v as u64),
        max_download_duration_secs: config.max_download_duration_secs.map(|v| v as u64),
        max_part_size_bytes: config.max_part_size_bytes.map(|v| v as u64),
        download_retry_policy: config.download_retry_policy,
        pipeline: config.pipeline,
        session_complete_pipeline: config.session_complete_pipeline,
        paired_segment_pipeline: config.paired_segment_pipeline,
        offline_check_count: config.offline_check_count.map(|v| v as u32),
        offline_check_delay_ms: config.offline_check_delay_ms.map(|v| v as u64),
    }
}

/// Default `?limit=` for the check-history strip — matches the screenshot's
/// "HISTORY (60PTS)" UI. The server caps requests at the writer's per-
/// streamer retention so a request asking for everything sees exactly what's
/// persisted.
const CHECK_HISTORY_DEFAULT_LIMIT: i64 = 60;
const CHECK_HISTORY_MAX_LIMIT: i64 = crate::database::repositories::KEEP_PER_STREAMER;

/// Query parameters for `GET /api/streamers/{id}/check-history`.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct CheckHistoryParams {
    /// Maximum rows to return. Defaults to 60. Server-clamped to the writer's
    /// per-streamer retention cap.
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/streamers/{id}/check-history",
    tag = "streamers",
    params(
        ("id" = String, Path, description = "Streamer ID"),
        CheckHistoryParams,
    ),
    responses(
        (status = 200, description = "Recent check-history rows, oldest first", body = StreamerCheckHistoryResponse),
        (status = 404, description = "Streamer not found", body = crate::api::error::ApiErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_check_history(
    State(state): State<StreamerRouteState>,
    Path(id): Path<String>,
    Query(params): Query<CheckHistoryParams>,
) -> ApiResult<Json<StreamerCheckHistoryResponse>> {
    // Confirm the streamer exists so a 404 is unambiguous (vs. "no rows yet"
    // for a brand-new streamer, which we want to render as an empty strip).
    live_streamer(&state.streamer_manager, &id)?;

    let limit = params
        .limit
        .unwrap_or(CHECK_HISTORY_DEFAULT_LIMIT)
        .clamp(1, CHECK_HISTORY_MAX_LIMIT);

    let rows = state
        .streamer_check_history_repository
        .list_recent(&id, limit)
        .await
        .map_err(ApiError::from)?;

    // Repository returns newest-first; reverse so the client renders
    // left → right = past → now without re-sorting.
    let mut items: Vec<StreamerCheckHistoryEntry> =
        rows.into_iter().rev().map(map_check_history_row).collect();

    // The reverse above costs nothing for typical 60-row payloads; an
    // in-place reverse would be marginally cheaper but harder to read.
    items.shrink_to_fit();

    Ok(Json(StreamerCheckHistoryResponse { items }))
}

/// Map one DB row to the wire format. A row with malformed JSON in
/// `stream_selected` or `streams_extracted_json` degrades to `None`
/// rather than dropping the row — the bar should still render (with the
/// outcome color), even if the tooltip's stream-list block is missing.
fn map_check_history_row(
    row: crate::database::models::StreamerCheckHistoryDbModel,
) -> StreamerCheckHistoryEntry {
    let stream_selected = row
        .stream_selected
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let streams_extracted_detail = row
        .streams_extracted_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    StreamerCheckHistoryEntry {
        checked_at: crate::database::time::ms_to_datetime(row.checked_at),
        duration_ms: row.duration_ms,
        outcome: row.outcome,
        fatal_kind: row.fatal_kind,
        filter_reason: row.filter_reason,
        error_message: row.error_message,
        streams_extracted: row.streams_extracted,
        stream_selected,
        streams_extracted_detail,
        title: row.title,
        category: row.category,
        viewer_count: row.viewer_count,
    }
}
