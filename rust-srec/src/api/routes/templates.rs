//! Template management routes.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateTemplateRequest, PaginatedResponse, PaginationParams, TemplateResponse,
    UpdateTemplateRequest,
};
use crate::api::server::AppState;
use crate::credentials::CredentialOwner;
use crate::database::models::TemplateConfigDbModel;
use crate::database::repositories::{StoredSelection, credential_selections};
use crate::utils::json::{self, JsonContext};
use tracing::info;

#[derive(Clone)]
pub struct TemplateRouteState {
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
    streamer_manager: std::sync::Arc<
        crate::streamer::StreamerManager<
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
}

impl FromRef<AppState> for TemplateRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
            streamer_manager: state.streamer_manager.clone(),
        }
    }
}

/// Request to clone a template.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CloneTemplateRequest {
    /// New name for the cloned template.
    pub new_name: String,
}

/// Create the templates router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_template))
        .route("/", get(list_templates))
        .route("/{id}", get(get_template))
        .route("/{id}", put(update_template))
        .route("/{id}", delete(delete_template))
        .route("/{id}/clone", post(clone_template))
}

/// Convert TemplateConfigDbModel to TemplateResponse, carrying the template's
/// stored account selections inside their platform overrides.
fn db_model_to_response(
    model: &TemplateConfigDbModel,
    usage_count: u32,
    selections: &[StoredSelection],
    proxy_route: crate::proxies::ProxyRoute,
) -> TemplateResponse {
    TemplateResponse {
        id: model.id.clone(),
        name: model.name.clone(),
        output_folder: model.output_folder.clone(),
        output_filename_template: model.output_filename_template.clone(),
        output_file_format: model.output_file_format.clone(),
        download_engine: model.download_engine.clone(),
        extractor: model.extractor.clone(),
        record_danmu: model.record_danmu,
        danmu_statistics: model.danmu_statistics.clone(),
        platform_overrides: credential_selections::inject_overrides(
            json::parse_optional_value_non_null(
                model.platform_overrides.as_deref(),
                JsonContext::TemplateField {
                    template_id: &model.id,
                    field: "platform_overrides",
                },
                "Invalid template JSON field; omitting from response",
            ),
            selections,
        ),
        engines_override: json::parse_optional_value_non_null(
            model.engines_override.as_deref(),
            JsonContext::TemplateField {
                template_id: &model.id,
                field: "engines_override",
            },
            "Invalid template JSON field; omitting from response",
        ),
        stream_selection_config: model.stream_selection_config.clone(),
        min_segment_size_bytes: model.min_segment_size_bytes,
        max_download_duration_secs: model.max_download_duration_secs,
        max_part_size_bytes: model.max_part_size_bytes,
        download_retry_policy: model.download_retry_policy.clone(),
        proxy_route,
        pipeline: model.pipeline.clone(),
        session_complete_pipeline: model.session_complete_pipeline.clone(),
        paired_segment_pipeline: model.paired_segment_pipeline.clone(),
        offline_check_count: model.offline_check_count,
        offline_check_delay_ms: model.offline_check_delay_ms,
        usage_count,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

/// The proxy route a template stores.
async fn template_route(
    config_service: &crate::config::ConfigService<
        crate::database::repositories::config::SqlxConfigRepository,
        crate::database::repositories::streamer::SqlxStreamerRepository,
    >,
    template_id: &str,
) -> ApiResult<crate::proxies::ProxyRoute> {
    Ok(config_service
        .proxy_route_of(
            &crate::database::repositories::proxies::RouteOwner::Template(template_id.to_owned()),
        )
        .await?)
}

/// The account selections a template stores, one per platform.
async fn template_selections(
    config_service: &crate::config::ConfigService<
        crate::database::repositories::config::SqlxConfigRepository,
        crate::database::repositories::streamer::SqlxStreamerRepository,
    >,
    template_id: &str,
) -> ApiResult<Vec<StoredSelection>> {
    config_service
        .credential_selections_for(&CredentialOwner::Template {
            template_id: template_id.to_owned(),
        })
        .await
        .map_err(ApiError::from)
}

/// Validate the optional offline-check overrides on a template request.
fn validate_offline_check_overrides(
    count: Option<i32>,
    delay_ms: Option<i64>,
) -> Result<(), ApiError> {
    if let Some(c) = count
        && c < 1
    {
        return Err(ApiError::validation("offline_check_count must be >= 1"));
    }
    if let Some(d) = delay_ms
        && d < 1_000
    {
        return Err(ApiError::validation(
            "offline_check_delay_ms must be >= 1000",
        ));
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/templates",
    tag = "templates",
    request_body = CreateTemplateRequest,
    responses(
        (status = 201, description = "Template created", body = TemplateResponse),
        (status = 422, description = "Validation error", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_template(
    State(state): State<TemplateRouteState>,
    Json(request): Json<CreateTemplateRequest>,
) -> ApiResult<(StatusCode, Json<TemplateResponse>)> {
    // Validate name
    if request.name.is_empty() {
        return Err(ApiError::validation("Template name cannot be empty"));
    }

    validate_offline_check_overrides(request.offline_check_count, request.offline_check_delay_ms)?;
    super::config::reject_proxy_config(request.proxy_config.as_ref())?;
    super::config::reject_cookies(request.cookies.as_ref())?;

    // Get config service from state
    let config_service = &state.config_service;

    // Create the template config model
    let mut template = TemplateConfigDbModel::new(&request.name);
    template.output_folder = request.output_folder;
    template.output_filename_template = request.output_filename_template;
    template.output_file_format = request.output_file_format;
    template.download_engine = request.download_engine;
    template.extractor = request.extractor;
    template.record_danmu = request.record_danmu;
    template.danmu_statistics = request.danmu_statistics;

    template.platform_overrides = match request.platform_overrides {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(ApiError::from)?),
        None => None,
    };
    template.engines_override = match request.engines_override {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(ApiError::from)?),
        None => None,
    };
    template.stream_selection_config = request.stream_selection_config;
    template.min_segment_size_bytes = request.min_segment_size_bytes;
    template.max_download_duration_secs = request.max_download_duration_secs;
    template.max_part_size_bytes = request.max_part_size_bytes;
    template.download_retry_policy = request.download_retry_policy;
    template.pipeline = request.pipeline;
    template.session_complete_pipeline = request.session_complete_pipeline;
    template.paired_segment_pipeline = request.paired_segment_pipeline;
    template.offline_check_count = request.offline_check_count;
    template.offline_check_delay_ms = request.offline_check_delay_ms;

    // Create the template
    config_service
        .create_template_config_with_route(&template, request.proxy_route.as_ref())
        .await
        .map_err(ApiError::from)?;
    let template = config_service
        .get_template_config(&template.id)
        .await
        .map_err(ApiError::from)?;
    let selections = template_selections(config_service, &template.id).await?;
    let route = template_route(config_service, &template.id).await?;

    Ok((
        StatusCode::CREATED,
        Json(db_model_to_response(&template, 0, &selections, route)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/templates",
    tag = "templates",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of templates", body = PaginatedResponse<TemplateResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_templates(
    State(state): State<TemplateRouteState>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<TemplateResponse>>> {
    // Get config service from state
    let config_service = &state.config_service;

    // Get streamer manager to count usage
    let streamer_manager = &state.streamer_manager;

    // Get all templates
    let templates = config_service
        .list_template_configs()
        .await
        .map_err(ApiError::from)?;

    let total = templates.len() as u64;

    // Apply pagination
    let offset = pagination.offset as usize;
    let effective_limit = pagination.limit.min(100);
    let limit = effective_limit as usize;

    let routes = config_service
        .proxy_routes_of_kind(
            &crate::database::repositories::proxies::RouteOwner::Template(String::new()),
        )
        .await?;
    let mut selections: HashMap<String, Vec<StoredSelection>> = HashMap::new();
    for stored in config_service
        .list_credential_selections()
        .await
        .map_err(ApiError::from)?
    {
        if let CredentialOwner::Template { template_id } = &stored.owner {
            selections
                .entry(template_id.clone())
                .or_default()
                .push(stored);
        }
    }

    let templates: Vec<TemplateResponse> = templates
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|t| {
            // Count streamers using this template
            let usage_count = streamer_manager.get_by_template(&t.id).len() as u32;
            let selections = selections.get(&t.id).map_or(&[][..], Vec::as_slice);
            let route = routes.get(&t.id).cloned().unwrap_or_default();
            db_model_to_response(&t, usage_count, selections, route)
        })
        .collect();

    let response = PaginatedResponse::new(templates, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template details", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_template(
    State(state): State<TemplateRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateResponse>> {
    // Get config service from state
    let config_service = &state.config_service;

    // Get streamer manager to count usage
    let streamer_manager = &state.streamer_manager;

    // Get the template
    let template = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    // Count streamers using this template
    let usage_count = streamer_manager.get_by_template(&id).len() as u32;
    let selections = template_selections(config_service, &id).await?;
    let route = template_route(config_service, &id).await?;

    Ok(Json(db_model_to_response(
        &template,
        usage_count,
        &selections,
        route,
    )))
}

#[utoipa::path(
    put,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    request_body = UpdateTemplateRequest,
    responses(
        (status = 200, description = "Template updated", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_template(
    State(state): State<TemplateRouteState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    validate_offline_check_overrides(request.offline_check_count, request.offline_check_delay_ms)?;
    super::config::reject_proxy_config(request.proxy_config.as_ref())?;
    super::config::reject_cookies(request.cookies.as_ref())?;

    // Get config service from state
    let config_service = &state.config_service;

    // Get streamer manager to count usage
    let streamer_manager = &state.streamer_manager;

    // Get the existing template
    let mut template = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    // Replace all fields (PUT semantics)
    if let Some(name) = request.name {
        if name.is_empty() {
            return Err(ApiError::validation("Template name cannot be empty"));
        }
        template.name = name;
    }

    // For optional fields, direct assignment handles both Some(v) and None (clearing)
    template.output_folder = request.output_folder;
    template.output_filename_template = request.output_filename_template;
    template.output_file_format = request.output_file_format;
    template.download_engine = request.download_engine;
    template.extractor = request.extractor;
    template.record_danmu = request.record_danmu;
    template.danmu_statistics = request.danmu_statistics;
    template.platform_overrides = match request.platform_overrides {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(ApiError::from)?),
        None => None,
    };
    template.engines_override = match request.engines_override {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(ApiError::from)?),
        None => None,
    };
    template.stream_selection_config = request.stream_selection_config;
    template.min_segment_size_bytes = request.min_segment_size_bytes;
    template.max_download_duration_secs = request.max_download_duration_secs;
    template.max_part_size_bytes = request.max_part_size_bytes;
    template.download_retry_policy = request.download_retry_policy;
    template.pipeline = request.pipeline;
    template.session_complete_pipeline = request.session_complete_pipeline;
    template.paired_segment_pipeline = request.paired_segment_pipeline;
    template.offline_check_count = request.offline_check_count;
    template.offline_check_delay_ms = request.offline_check_delay_ms;

    // Update the template
    config_service
        .update_template_config_with_route(&template, request.proxy_route.as_ref())
        .await
        .map_err(ApiError::from)?;
    // Selections the request omitted are kept, so read back what is stored.
    let template = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    info!("Updated template '{}' (id: {})", template.name, id);

    // Count streamers using this template
    let usage_count = streamer_manager.get_by_template(&id).len() as u32;
    let selections = template_selections(config_service, &id).await?;
    let route = template_route(config_service, &id).await?;

    Ok(Json(db_model_to_response(
        &template,
        usage_count,
        &selections,
        route,
    )))
}

#[utoipa::path(
    delete,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template deleted", body = crate::api::openapi::MessageResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "Template in use", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_template(
    State(state): State<TemplateRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get config service from state
    let config_service = &state.config_service;

    // Get streamer manager to check usage
    let streamer_manager = &state.streamer_manager;

    // Check if template exists
    config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    // Check if any streamers are using this template. The guard counts rows
    // rather than visible streamers: `streamers.template_config_id` is a
    // foreign key with no `ON DELETE` action, so a streamer that is marked
    // deleted and waiting for `StreamerManager::reap_deleted` still makes
    // SQLite refuse the delete.
    let streamers_using = streamer_manager.get_by_template(&id).len();
    let rows_referencing = streamer_manager.template_reference_count(&id);
    if streamers_using > 0 {
        return Err(ApiError::conflict(format!(
            "Cannot delete template '{}': {} streamer(s) are using it",
            id, streamers_using
        )));
    }
    if rows_referencing > 0 {
        return Err(ApiError::conflict(format!(
            "Cannot delete template '{}': a streamer using it is still being removed; try again shortly",
            id
        )));
    }

    // Delete the template
    config_service
        .delete_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Template '{}' deleted successfully", id)
    })))
}

#[utoipa::path(
    post,
    path = "/api/templates/{id}/clone",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID to clone")),
    request_body = CloneTemplateRequest,
    responses(
        (status = 201, description = "Template cloned", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "Template name already exists", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn clone_template(
    State(state): State<TemplateRouteState>,
    Path(id): Path<String>,
    Json(request): Json<CloneTemplateRequest>,
) -> ApiResult<(StatusCode, Json<TemplateResponse>)> {
    // Validate new name
    if request.new_name.is_empty() {
        return Err(ApiError::validation("Template name cannot be empty"));
    }

    // Get config service from state
    let config_service = &state.config_service;

    // Get the existing template
    let existing = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    // Check if a template with the new name already exists
    if config_service
        .get_template_config_by_name(&request.new_name)
        .await
        .is_ok()
    {
        return Err(ApiError::conflict(format!(
            "A template with name '{}' already exists",
            request.new_name
        )));
    }

    let cloned = config_service
        .clone_template_config(&id, &request.new_name)
        .await
        .map_err(ApiError::from)?;

    info!(
        "Cloned template '{}' (id: {}) to '{}' (id: {})",
        existing.name, id, cloned.name, cloned.id
    );

    let selections = template_selections(config_service, &cloned.id).await?;
    let route = template_route(config_service, &cloned.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(db_model_to_response(&cloned, 0, &selections, route)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn templates_store_a_route_and_refuse_the_replaced_proxy_config() {
        use crate::config::{ConfigEventBroadcaster, ConfigService};
        use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};
        use axum::http::StatusCode;
        use std::sync::Arc;
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let proxy =
            crate::database::repositories::proxies::save_for_test(&pool, "T", "http://t.example:1")
                .await;
        let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let state = TemplateRouteState {
            config_service: Arc::new(ConfigService::new(
                Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
                streamers.clone(),
            )),
            streamer_manager: Arc::new(crate::streamer::StreamerManager::new(
                streamers,
                ConfigEventBroadcaster::new(),
            )),
        };
        let (status, Json(created)) = create_template(
            State(state.clone()),
            Json(
                serde_json::from_value(serde_json::json!({
                    "name": "Routed",
                    "proxy_route": {"kind": "proxy", "id": proxy},
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            created.proxy_route,
            crate::proxies::ProxyRoute::Proxy { id: proxy.clone() }
        );
        // An update without a route keeps the stored one.
        let Json(updated) = update_template(
            State(state.clone()),
            Path(created.id.clone()),
            Json(serde_json::from_value(serde_json::json!({"name": "Renamed"})).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(updated.proxy_route, created.proxy_route);
        for body in [
            serde_json::json!({"name": "Legacy", "proxy_config": "{\"enabled\":false}"}),
            serde_json::json!({"name": "Legacy", "platform_overrides": {"bilibili": {"proxy_config": {"enabled": true}}}}),
        ] {
            let error = create_template(
                State(state.clone()),
                Json(serde_json::from_value(body).unwrap()),
            )
            .await
            .unwrap_err();
            assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(error.code, "PROXY_CONFIG_REPLACED");
        }
        let error = update_template(
            State(state.clone()),
            Path(created.id.clone()),
            Json(
                serde_json::from_value(
                    serde_json::json!({"proxy_route": {"kind": "proxy", "id": "missing"}}),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PROXY_NOT_FOUND");

        // Cookies on a template are refused; a blank value asks for none.
        let error = create_template(
            State(state.clone()),
            Json(
                serde_json::from_value(serde_json::json!({"name": "Cookies", "cookies": "a=b"}))
                    .unwrap(),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (error.status, error.code.as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, "COOKIES_REPLACED")
        );
        let error = update_template(
            State(state.clone()),
            Path(created.id.clone()),
            Json(serde_json::from_value(serde_json::json!({"cookies": "a=b"})).unwrap()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "COOKIES_REPLACED");
        let Json(blank) = update_template(
            State(state.clone()),
            Path(created.id.clone()),
            Json(serde_json::from_value(serde_json::json!({"cookies": " "})).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(blank.id, created.id);
    }

    #[tokio::test]
    async fn template_routes_distinguish_typed_not_found_from_database_error_text() {
        use crate::config::{ConfigEventBroadcaster, ConfigService};
        use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};
        use axum::http::StatusCode;
        use std::sync::Arc;
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE template_config (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let state = TemplateRouteState {
            config_service: Arc::new(ConfigService::new(
                Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
                streamers.clone(),
            )),
            streamer_manager: Arc::new(crate::streamer::StreamerManager::new(
                streamers,
                ConfigEventBroadcaster::new(),
            )),
        };
        for status in [StatusCode::NOT_FOUND, StatusCode::INTERNAL_SERVER_ERROR] {
            let errors = [
                get_template(State(state.clone()), Path("missing".to_string()))
                    .await
                    .unwrap_err(),
                update_template(
                    State(state.clone()),
                    Path("missing".to_string()),
                    Json(serde_json::from_value(serde_json::json!({})).unwrap()),
                )
                .await
                .unwrap_err(),
                delete_template(State(state.clone()), Path("missing".to_string()))
                    .await
                    .unwrap_err(),
                clone_template(
                    State(state.clone()),
                    Path("missing".to_string()),
                    Json(CloneTemplateRequest {
                        new_name: "cloned".to_string(),
                    }),
                )
                .await
                .unwrap_err(),
            ];
            for error in errors {
                assert_eq!(error.status, status);
                if status == StatusCode::INTERNAL_SERVER_ERROR {
                    assert_eq!(error.message, "Database error occurred");
                }
            }
            if status == StatusCode::NOT_FOUND {
                sqlx::query("DROP TABLE template_config")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("CREATE VIEW template_config AS SELECT * FROM \"not found\"")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        pool.close().await;
    }

    #[test]
    fn test_template_response_serialization() {
        let response = TemplateResponse {
            id: "123".to_string(),
            name: "Test Template".to_string(),
            output_folder: Some("/downloads".to_string()),
            output_filename_template: None,
            output_file_format: Some("mp4".to_string()),
            download_engine: None,
            extractor: None,
            record_danmu: Some(true),
            danmu_statistics: None,
            platform_overrides: None,
            engines_override: None,
            stream_selection_config: None,
            min_segment_size_bytes: None,
            max_download_duration_secs: None,
            max_part_size_bytes: None,
            download_retry_policy: None,
            proxy_route: crate::proxies::ProxyRoute::Inherit,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            offline_check_count: None,
            offline_check_delay_ms: None,
            usage_count: 5,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("Test Template"));
        assert!(json.contains("mp4"));
    }

    #[test]
    fn test_db_model_to_response() {
        let mut model = TemplateConfigDbModel::new("test");
        let at = chrono::DateTime::from_timestamp_millis(1_767_323_045_123).unwrap();
        model.created_at = at;
        model.updated_at = at;
        let response = db_model_to_response(&model, 3, &[], crate::proxies::ProxyRoute::Inherit);

        assert_eq!(response.name, "test");
        assert_eq!(response.usage_count, 3);
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["created_at"], "2026-01-02T03:04:05.123Z");
        assert_eq!(json["updated_at"], "2026-01-02T03:04:05.123Z");
    }
}
