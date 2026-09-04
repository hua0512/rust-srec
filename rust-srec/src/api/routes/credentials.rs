//! Credential management routes.

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::credentials::platforms::bilibili::{BilibiliCredentialManager, QrPollStatus};
use crate::credentials::{CredentialScope, CredentialSource};
use crate::streamer::{
    StreamerMetadata,
    manager::{ReloadPublish, StreamerUpdateParams},
};

/// The `StreamerManager` instantiation carried by [`CredentialRouteState`].
type CredentialStreamerManager = crate::streamer::StreamerManager<
    crate::database::repositories::streamer::SqlxStreamerRepository,
>;

/// The `ConfigService` instantiation carried by [`CredentialRouteState`].
type CredentialConfigService = crate::config::ConfigService<
    crate::database::repositories::config::SqlxConfigRepository,
    crate::database::repositories::streamer::SqlxStreamerRepository,
>;

#[derive(Clone)]
pub struct CredentialRouteState {
    config_service: std::sync::Arc<CredentialConfigService>,
    credential_service: std::sync::Arc<
        crate::credentials::CredentialRefreshService<
            crate::database::repositories::config::SqlxConfigRepository,
        >,
    >,
    /// Streamer-scoped credential writes go through the manager rather than a
    /// `StreamerRepository`: `StreamerManager::partial_update_streamer` rebuilds the whole
    /// `streamers` row from the manager's metadata cache, so that cache has to carry the
    /// credentials for them to survive the next streamer edit.
    streamer_manager: std::sync::Arc<CredentialStreamerManager>,
}

impl FromRef<AppState> for CredentialRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
            credential_service: state.credential_service.clone(),
            streamer_manager: state.streamer_manager.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CredentialSourceResponse {
    pub platform: String,
    pub scope_type: String,
    pub scope_id: String,
    pub scope_name: String,
    pub has_refresh_token: bool,
    pub cookie_length: usize,
}

impl CredentialSourceResponse {
    fn from_source(source: &CredentialSource) -> Self {
        let (scope_type, scope_id, scope_name) = match &source.scope {
            CredentialScope::Platform {
                platform_id,
                platform_name,
            } => ("platform", platform_id.as_str(), platform_name.as_str()),
            CredentialScope::Template {
                template_id,
                template_name,
            } => ("template", template_id.as_str(), template_name.as_str()),
            CredentialScope::Streamer {
                streamer_id,
                streamer_name,
            } => ("streamer", streamer_id.as_str(), streamer_name.as_str()),
        };

        Self {
            platform: source.platform_name.clone(),
            scope_type: scope_type.to_string(),
            scope_id: scope_id.to_string(),
            scope_name: scope_name.to_string(),
            has_refresh_token: source.has_refresh_token(),
            cookie_length: source.cookies.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CredentialRefreshResponse {
    pub refreshed: bool,
    pub requires_relogin: bool,
    pub source: Option<CredentialSourceResponse>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QrGenerateApiResponse {
    pub url: String,
    pub auth_code: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialSaveScope {
    Platform { id: String },
    Template { id: String },
    Streamer { id: String },
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct QrPollRequest {
    pub auth_code: String,
    pub scope: CredentialSaveScope,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QrPollApiResponse {
    pub status: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TemplateCredentialQuery {
    /// Optional platform name hint, required when a template is used for multiple platforms.
    pub platform: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/streamers/{id}/source",
            get(get_streamer_credential_source),
        )
        .route(
            "/streamers/{id}/refresh",
            post(refresh_streamer_credentials),
        )
        .route(
            "/platforms/{id}/source",
            get(get_platform_credential_source),
        )
        .route(
            "/platforms/{id}/refresh",
            post(refresh_platform_credentials),
        )
        .route(
            "/templates/{id}/source",
            get(get_template_credential_source),
        )
        .route(
            "/templates/{id}/refresh",
            post(refresh_template_credentials),
        )
        .route("/bilibili/qr/generate", post(bilibili_qr_generate))
        .route("/bilibili/qr/poll", post(bilibili_qr_poll))
}

fn extract_platform_refresh_token(platform_specific_config: Option<&str>) -> Option<String> {
    platform_specific_config
        .and_then(|config| serde_json::from_str::<serde_json::Value>(config).ok())
        .and_then(|v| {
            v.get("refresh_token")
                .and_then(|t| t.as_str())
                .map(String::from)
        })
}

fn extract_platform_access_token(platform_specific_config: Option<&str>) -> Option<String> {
    platform_specific_config
        .and_then(|config| serde_json::from_str::<serde_json::Value>(config).ok())
        .and_then(|v| {
            v.get("access_token")
                .and_then(|t| t.as_str())
                .map(String::from)
        })
}

fn extract_template_refresh_token(
    platform_overrides: Option<&str>,
    platform_name: &str,
) -> Option<String> {
    platform_overrides
        .and_then(|overrides| serde_json::from_str::<serde_json::Value>(overrides).ok())
        .and_then(|v| v.get(platform_name).cloned())
        .and_then(|p| p.get("refresh_token").cloned())
        .and_then(|t| t.as_str().map(String::from))
}

fn extract_template_access_token(
    platform_overrides: Option<&str>,
    platform_name: &str,
) -> Option<String> {
    platform_overrides
        .and_then(|overrides| serde_json::from_str::<serde_json::Value>(overrides).ok())
        .and_then(|v| v.get(platform_name).cloned())
        .and_then(|p| p.get("access_token").cloned())
        .and_then(|t| t.as_str().map(String::from))
}

fn infer_template_platform_name(
    platform_overrides: Option<&str>,
    platform_hint: Option<&str>,
) -> Option<String> {
    if let Some(hint) = platform_hint
        && !hint.trim().is_empty()
    {
        return Some(hint.to_string());
    }

    let overrides = platform_overrides
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let mut keys_with_refresh_token: Vec<String> = overrides
        .iter()
        .filter_map(|(key, value)| {
            value
                .get("refresh_token")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
                .map(|_| key.clone())
        })
        .collect();
    keys_with_refresh_token.sort();
    keys_with_refresh_token.dedup();

    if keys_with_refresh_token.len() == 1 {
        return keys_with_refresh_token.into_iter().next();
    }

    let mut keys: Vec<String> = overrides.keys().cloned().collect();
    keys.sort();
    keys.dedup();

    if keys.len() == 1 {
        return keys.into_iter().next();
    }

    if overrides.contains_key("bilibili") {
        return Some("bilibili".to_string());
    }

    None
}

#[utoipa::path(
    get,
    path = "/api/credentials/streamers/{id}/source",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 200, description = "Credential source", body = CredentialSourceResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_streamer_credential_source(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = &state.config_service;

    let context = config_service
        .get_context_for_streamer(&id)
        .await
        .map_err(ApiError::from)?;

    let source = context.credential_source.as_ref().ok_or_else(|| {
        ApiError::not_found(format!("No credentials configured for streamer {id}"))
    })?;

    Ok(Json(CredentialSourceResponse::from_source(source)))
}

#[utoipa::path(
    get,
    path = "/api/credentials/platforms/{id}/source",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Platform config id")
    ),
    responses(
        (status = 200, description = "Credential source", body = CredentialSourceResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_platform_credential_source(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = &state.config_service;

    let platform = config_service
        .get_platform_config(&id)
        .await
        .map_err(ApiError::from)?;

    let cookies = platform.cookies.as_deref().unwrap_or_default().trim();
    if cookies.is_empty() {
        return Err(ApiError::not_found(format!(
            "No credentials configured for platform {id}"
        )));
    }

    let refresh_token =
        extract_platform_refresh_token(platform.platform_specific_config.as_deref());
    let access_token = extract_platform_access_token(platform.platform_specific_config.as_deref());
    let source = CredentialSource::new(
        CredentialScope::Platform {
            platform_id: platform.id,
            platform_name: platform.platform_name.clone(),
        },
        cookies.to_string(),
        refresh_token,
        platform.platform_name,
    )
    .with_access_token(access_token);

    Ok(Json(CredentialSourceResponse::from_source(&source)))
}

#[utoipa::path(
    get,
    path = "/api/credentials/templates/{id}/source",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Template config id"),
        ("platform" = Option<String>, Query, description = "Optional platform name hint when a template is shared across platforms")
    ),
    responses(
        (status = 200, description = "Credential source", body = CredentialSourceResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_template_credential_source(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
    Query(query): Query<TemplateCredentialQuery>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = &state.config_service;

    let template = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    let cookies = template.cookies.as_deref().unwrap_or_default().trim();
    if cookies.is_empty() {
        return Err(ApiError::not_found(format!(
            "No credentials configured for template {id}"
        )));
    }

    let platform_name = infer_template_platform_name(
        template.platform_overrides.as_deref(),
        query.platform.as_deref(),
    )
    .unwrap_or_else(|| "unknown".to_string());
    let refresh_token =
        extract_template_refresh_token(template.platform_overrides.as_deref(), &platform_name);
    let access_token =
        extract_template_access_token(template.platform_overrides.as_deref(), &platform_name);

    let source = CredentialSource::new(
        CredentialScope::Template {
            template_id: template.id,
            template_name: template.name,
        },
        cookies.to_string(),
        refresh_token,
        platform_name,
    )
    .with_access_token(access_token);

    Ok(Json(CredentialSourceResponse::from_source(&source)))
}

#[utoipa::path(
    post,
    path = "/api/credentials/streamers/{id}/refresh",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Streamer id")
    ),
    responses(
        (status = 200, description = "Refresh result", body = CredentialRefreshResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn refresh_streamer_credentials(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = &state.config_service;
    let credential_service = &state.credential_service;

    let context = config_service
        .get_context_for_streamer(&id)
        .await
        .map_err(ApiError::from)?;
    let source = context.credential_source.as_ref().ok_or_else(|| {
        ApiError::not_found(format!("No credentials configured for streamer {id}"))
    })?;

    match credential_service.check_and_refresh_source(source).await {
        Ok(Some(_new_cookies)) => {
            // Invalidate caches affected by the updated scope.
            match &source.scope {
                CredentialScope::Streamer { streamer_id, .. } => {
                    config_service.invalidate_streamer(streamer_id);
                    // `CredentialStore::update_credentials` rewrites `streamer_specific_config`
                    // with its own SQL, leaving the previous cookies in the manager's metadata
                    // cache, which `partial_update_streamer` would write back over the refreshed
                    // ones. Only that column changed, so `reload_from_repo` publishes no event.
                    // A failure here leaves the row correct and the cache stale, which the next
                    // reload repairs, so it is logged rather than failing a refresh that landed.
                    if let Err(error) = state
                        .streamer_manager
                        .reload_from_repo(streamer_id, ReloadPublish::StateOnly)
                        .await
                    {
                        tracing::warn!(
                            streamer_id = %streamer_id,
                            %error,
                            "Failed to reload streamer after credential refresh; cache may be stale"
                        );
                    }
                }
                CredentialScope::Template { template_id, .. } => {
                    config_service
                        .invalidate_template(template_id)
                        .await
                        .map_err(ApiError::from)?;
                }
                CredentialScope::Platform { platform_id, .. } => {
                    config_service
                        .invalidate_platform(platform_id)
                        .await
                        .map_err(ApiError::from)?;
                }
            }

            Ok(Json(CredentialRefreshResponse {
                refreshed: true,
                requires_relogin: false,
                source: Some(CredentialSourceResponse::from_source(source)),
            }))
        }
        Ok(None) => Ok(Json(CredentialRefreshResponse {
            refreshed: false,
            requires_relogin: false,
            source: Some(CredentialSourceResponse::from_source(source)),
        })),
        Err(e) => Err(ApiError::bad_request(format!(
            "{} (requires_relogin={})",
            e,
            e.requires_relogin()
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/credentials/platforms/{id}/refresh",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Platform config id")
    ),
    responses(
        (status = 200, description = "Refresh result", body = CredentialRefreshResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn refresh_platform_credentials(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = &state.config_service;
    let credential_service = &state.credential_service;

    let platform = config_service
        .get_platform_config(&id)
        .await
        .map_err(ApiError::from)?;

    let cookies = platform.cookies.as_deref().unwrap_or_default().trim();
    if cookies.is_empty() {
        return Err(ApiError::not_found(format!(
            "No credentials configured for platform {id}"
        )));
    }

    let refresh_token =
        extract_platform_refresh_token(platform.platform_specific_config.as_deref());
    let access_token = extract_platform_access_token(platform.platform_specific_config.as_deref());
    let source = CredentialSource::new(
        CredentialScope::Platform {
            platform_id: platform.id.clone(),
            platform_name: platform.platform_name.clone(),
        },
        cookies.to_string(),
        refresh_token,
        platform.platform_name,
    )
    .with_access_token(access_token);

    match credential_service.check_and_refresh_source(&source).await {
        Ok(Some(_new_cookies)) => {
            config_service
                .invalidate_platform(&platform.id)
                .await
                .map_err(ApiError::from)?;
            Ok(Json(CredentialRefreshResponse {
                refreshed: true,
                requires_relogin: false,
                source: Some(CredentialSourceResponse::from_source(&source)),
            }))
        }
        Ok(None) => Ok(Json(CredentialRefreshResponse {
            refreshed: false,
            requires_relogin: false,
            source: Some(CredentialSourceResponse::from_source(&source)),
        })),
        Err(e) => Err(ApiError::bad_request(format!(
            "{} (requires_relogin={})",
            e,
            e.requires_relogin()
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/credentials/templates/{id}/refresh",
    tag = "credentials",
    params(
        ("id" = String, Path, description = "Template config id"),
        ("platform" = Option<String>, Query, description = "Optional platform name hint when a template is shared across platforms")
    ),
    responses(
        (status = 200, description = "Refresh result", body = CredentialRefreshResponse),
        (status = 404, description = "No credentials configured", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn refresh_template_credentials(
    State(state): State<CredentialRouteState>,
    Path(id): Path<String>,
    Query(query): Query<TemplateCredentialQuery>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = &state.config_service;
    let credential_service = &state.credential_service;

    let template = config_service
        .get_template_config(&id)
        .await
        .map_err(ApiError::from)?;

    let cookies = template.cookies.as_deref().unwrap_or_default().trim();
    if cookies.is_empty() {
        return Err(ApiError::not_found(format!(
            "No credentials configured for template {id}"
        )));
    }

    let platform_name = infer_template_platform_name(
        template.platform_overrides.as_deref(),
        query.platform.as_deref(),
    )
    .ok_or_else(|| {
        ApiError::bad_request(
            "Template platform is ambiguous; pass ?platform=<platform_name> to refresh".to_string(),
        )
    })?;
    let refresh_token =
        extract_template_refresh_token(template.platform_overrides.as_deref(), &platform_name);
    let access_token =
        extract_template_access_token(template.platform_overrides.as_deref(), &platform_name);

    let source = CredentialSource::new(
        CredentialScope::Template {
            template_id: template.id.clone(),
            template_name: template.name,
        },
        cookies.to_string(),
        refresh_token,
        platform_name,
    )
    .with_access_token(access_token);

    match credential_service.check_and_refresh_source(&source).await {
        Ok(Some(_new_cookies)) => {
            config_service
                .invalidate_template(&template.id)
                .await
                .map_err(ApiError::from)?;
            Ok(Json(CredentialRefreshResponse {
                refreshed: true,
                requires_relogin: false,
                source: Some(CredentialSourceResponse::from_source(&source)),
            }))
        }
        Ok(None) => Ok(Json(CredentialRefreshResponse {
            refreshed: false,
            requires_relogin: false,
            source: Some(CredentialSourceResponse::from_source(&source)),
        })),
        Err(e) => Err(ApiError::bad_request(format!(
            "{} (requires_relogin={})",
            e,
            e.requires_relogin()
        ))),
    }
}

/// Merge freshly issued credentials into a streamer's `streamer_specific_config` document.
///
/// Keys other than `cookies`, `refresh_token` and `access_token` are preserved. A document that
/// is missing, unparseable, or not a JSON object is replaced by a fresh object.
fn merge_streamer_credentials(
    existing: Option<&str>,
    cookies: &str,
    refresh_token: &str,
    access_token: Option<&str>,
) -> ApiResult<String> {
    let mut config: serde_json::Value = existing
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object() {
        config = serde_json::json!({});
    }
    if let Some(map) = config.as_object_mut() {
        map.insert(
            "cookies".to_string(),
            serde_json::Value::String(cookies.to_string()),
        );
        map.insert(
            "refresh_token".to_string(),
            serde_json::Value::String(refresh_token.to_string()),
        );
        if let Some(at) = access_token {
            map.insert(
                "access_token".to_string(),
                serde_json::Value::String(at.to_string()),
            );
        }
    }

    serde_json::to_string(&config)
        .map_err(|error| ApiError::internal(format!("Failed to save credentials: {error}")))
}

/// Save a completed bilibili QR login onto streamer `id`, returning the updated metadata.
///
/// The document the credentials are merged into comes from the manager's metadata cache, which
/// `StreamerManager` maintains as the runtime source of truth for registered streamers, so an
/// `id` missing from it is reported as not found rather than read back from the `streamers` table.
///
/// Rejects a streamer whose `platform_config_id` does not resolve to a bilibili platform config,
/// since the credentials only make sense to the bilibili extractor.
///
/// `StreamerManager::partial_update_streamer` is the write path because it is the one that keeps
/// the manager's metadata cache and the `streamers` row in step: it rebuilds the whole row from
/// that cache, which therefore has to hold the credentials for the next streamer edit to preserve
/// them.
///
/// The `ConfigUpdateEvent::StreamerMetadataUpdated` that write publishes does not carry the
/// cookies to the streamer's actor: `StreamerConfig` holds only check intervals, priority and
/// `batch_capable`. Downloads read cookies through `ConfigService::get_config_for_streamer`, so
/// `ConfigService::invalidate_streamer` below is the call that makes them visible. The event has
/// two handlers, both harmless here:
///
/// - `services::container::events` re-invalidates the merged config, calls
///   `refresh_metadata_offline_check`, and for a streamer that is not
///   `StreamerMetadata::is_active` runs `handle_streamer_disabled` — nothing to stop in those
///   states, since no download or danmu collection is running.
/// - `scheduler::service::handle_config_event` routes the update and calls
///   `ensure_streamer_actor_state`, which removes the actor of an inactive streamer —
///   `handle_state_sync` already removed it when the streamer entered that state.
async fn save_streamer_credentials(
    config_service: &CredentialConfigService,
    streamer_manager: &CredentialStreamerManager,
    id: &str,
    cookies: &str,
    refresh_token: &str,
    access_token: Option<&str>,
) -> ApiResult<StreamerMetadata> {
    // A streamer carrying `deleted_at` is gone as far as the API is concerned,
    // and `StreamerManager::reap_deleted` is about to drop the row this would
    // write `streamer_specific_config` to.
    let metadata = streamer_manager
        .get_streamer(id)
        .filter(|metadata| !metadata.is_deleted())
        .ok_or_else(|| ApiError::not_found(format!("Streamer with id '{id}' not found")))?;

    let platform = config_service
        .get_platform_config(&metadata.platform_config_id)
        .await
        .map_err(ApiError::from)?;
    if !platform.platform_name.eq_ignore_ascii_case("bilibili") {
        return Err(ApiError::bad_request(format!(
            "Streamer {id} is not bilibili"
        )));
    }

    let streamer_specific_config = merge_streamer_credentials(
        metadata.streamer_specific_config.as_deref(),
        cookies,
        refresh_token,
        access_token,
    )?;

    let updated = streamer_manager
        .partial_update_streamer(StreamerUpdateParams {
            id: metadata.id.clone(),
            name: None,
            url: None,
            platform_config_id: None,
            template_config_id: None,
            priority: None,
            state: None,
            streamer_specific_config: Some(Some(streamer_specific_config)),
        })
        .await
        .map_err(ApiError::from)?;

    config_service.invalidate_streamer(id);

    Ok(updated)
}

fn bilibili_qr_manager() -> Result<BilibiliCredentialManager, ApiError> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new).clone();
    BilibiliCredentialManager::new(client)
        .map_err(|error| ApiError::internal(format!("Failed to create manager: {error}")))
}

#[utoipa::path(
    post,
    path = "/api/credentials/bilibili/qr/generate",
    tag = "credentials",
    responses(
        (status = 200, description = "QR code generated", body = QrGenerateApiResponse),
        (status = 500, description = "Failed", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn bilibili_qr_generate() -> ApiResult<Json<QrGenerateApiResponse>> {
    let manager = bilibili_qr_manager()?;

    let result = manager
        .generate_qr()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to generate QR: {}", e)))?;

    Ok(Json(QrGenerateApiResponse {
        url: result.url,
        auth_code: result.auth_code,
    }))
}

#[utoipa::path(
    post,
    path = "/api/credentials/bilibili/qr/poll",
    tag = "credentials",
    request_body = QrPollRequest,
    responses(
        (status = 200, description = "Poll result", body = QrPollApiResponse),
        (status = 500, description = "Failed", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn bilibili_qr_poll(
    State(state): State<CredentialRouteState>,
    Json(body): Json<QrPollRequest>,
) -> ApiResult<Json<QrPollApiResponse>> {
    let manager = bilibili_qr_manager()?;

    let result = manager
        .poll_qr(&body.auth_code)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to poll QR: {}", e)))?;

    let api_message = if !result.message.is_empty() {
        result.message.as_str()
    } else {
        ""
    };

    let (status, message) = match result.status {
        QrPollStatus::NotScanned => (
            "not_scanned",
            if !api_message.is_empty() {
                api_message
            } else {
                "Waiting for scan"
            },
        ),
        QrPollStatus::ScannedNotConfirmed => (
            "scanned",
            if !api_message.is_empty() {
                api_message
            } else {
                "Scanned, waiting for confirmation"
            },
        ),
        QrPollStatus::Expired => ("expired", "QR code expired"),
        QrPollStatus::Success => ("success", "Login successful"),
    };

    let mut saved_scope: Option<CredentialScope> = None;

    if result.status == QrPollStatus::Success
        && let (Some(cookies), Some(refresh_token)) = (&result.cookies, &result.refresh_token)
    {
        let access_token = result.access_token.as_deref();

        match &body.scope {
            CredentialSaveScope::Platform { id } => {
                let cs = &state.config_service;
                let mut platform = cs.get_platform_config(id).await.map_err(ApiError::from)?;

                if !platform.platform_name.eq_ignore_ascii_case("bilibili") {
                    return Err(ApiError::bad_request(format!(
                        "Platform {id} is not bilibili"
                    )));
                }

                platform.cookies = Some(cookies.clone());

                let mut specific: serde_json::Value = platform
                    .platform_specific_config
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                if !specific.is_object() {
                    specific = serde_json::json!({});
                }
                if let Some(map) = specific.as_object_mut() {
                    map.insert(
                        "refresh_token".to_string(),
                        serde_json::Value::String(refresh_token.clone()),
                    );
                    if let Some(at) = access_token {
                        map.insert(
                            "access_token".to_string(),
                            serde_json::Value::String(at.to_string()),
                        );
                    }
                    map.insert(
                        "last_cookie_check_date".to_string(),
                        serde_json::Value::String(
                            chrono::Utc::now().format("%Y-%m-%d").to_string(),
                        ),
                    );
                    map.insert(
                        "last_cookie_check_result".to_string(),
                        serde_json::Value::String("valid".to_string()),
                    );
                }

                platform.platform_specific_config =
                    Some(serde_json::to_string(&specific).unwrap_or_default());

                cs.update_platform_config(&platform)
                    .await
                    .map_err(ApiError::from)?;
                cs.invalidate_platform(id).await.map_err(ApiError::from)?;
                tracing::info!(platform_id = %id, "Saved QR credentials to platform");

                saved_scope = Some(CredentialScope::Platform {
                    platform_id: id.clone(),
                    platform_name: platform.platform_name.clone(),
                });
            }
            CredentialSaveScope::Template { id } => {
                let cs = &state.config_service;
                let mut template = cs.get_template_config(id).await.map_err(ApiError::from)?;

                template.cookies = Some(cookies.clone());

                let mut overrides: serde_json::Value = template
                    .platform_overrides
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                if !overrides.is_object() {
                    overrides = serde_json::json!({});
                }
                if let Some(root) = overrides.as_object_mut() {
                    let entry = root
                        .entry("bilibili".to_string())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if !entry.is_object() {
                        *entry = serde_json::Value::Object(serde_json::Map::new());
                    }
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert(
                            "refresh_token".to_string(),
                            serde_json::Value::String(refresh_token.clone()),
                        );
                        if let Some(at) = access_token {
                            obj.insert(
                                "access_token".to_string(),
                                serde_json::Value::String(at.to_string()),
                            );
                        }
                    }
                }

                template.platform_overrides =
                    Some(serde_json::to_string(&overrides).unwrap_or_default());
                cs.update_template_config(&template)
                    .await
                    .map_err(ApiError::from)?;
                cs.invalidate_template(id).await.map_err(ApiError::from)?;
                tracing::info!(template_id = %id, "Saved QR credentials to template");

                saved_scope = Some(CredentialScope::Template {
                    template_id: id.clone(),
                    template_name: template.name.clone(),
                });
            }
            CredentialSaveScope::Streamer { id } => {
                let metadata = save_streamer_credentials(
                    &state.config_service,
                    &state.streamer_manager,
                    id,
                    cookies,
                    refresh_token,
                    access_token,
                )
                .await?;
                tracing::info!(streamer_id = %id, "Saved QR credentials to streamer");

                saved_scope = Some(CredentialScope::Streamer {
                    streamer_id: id.clone(),
                    streamer_name: metadata.name,
                });
            }
        }
    }

    if let Some(scope) = &saved_scope {
        state.credential_service.invalidate(scope);
    }

    Ok(Json(QrPollApiResponse {
        status: status.to_string(),
        success: result.status == QrPollStatus::Success,
        message: message.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigEventBroadcaster, ConfigService};
    use crate::credentials::test_support::StubCredentialManager;
    use crate::credentials::{CredentialRefreshService, CredentialResolver};
    use crate::database::models::StreamerDbModel;
    use crate::database::repositories::{
        SqlxConfigRepository, SqlxCredentialStore, SqlxStreamerRepository, StreamerRepository as _,
    };
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::streamer::StreamerManager;
    use sqlx::SqlitePool;
    use std::sync::Arc;

    const STREAMER_ID: &str = "streamer-under-test";

    /// The pieces `bilibili_qr_poll` and `refresh_streamer_credentials` reach for, over one
    /// in-memory database holding a single streamer on `platform_config_id`.
    ///
    /// `platform-bilibili` and `platform-douyin` are rows seeded by the initial schema migration.
    struct Harness {
        pool: SqlitePool,
        repo: Arc<SqlxStreamerRepository>,
        config_repo: Arc<SqlxConfigRepository>,
        manager: Arc<CredentialStreamerManager>,
        config_service: Arc<CredentialConfigService>,
    }

    async fn harness(platform_config_id: &str, existing: Option<&str>) -> Harness {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let mut model = StreamerDbModel::new(
            "Streamer",
            "https://live.bilibili.com/1",
            platform_config_id,
        );
        model.id = STREAMER_ID.to_string();
        model.streamer_specific_config = existing.map(str::to_string);

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        repo.create_streamer(&model).await.unwrap();

        let manager = Arc::new(StreamerManager::new(
            repo.clone(),
            ConfigEventBroadcaster::new(),
        ));
        manager.hydrate().await.unwrap();

        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let config_service = Arc::new(ConfigService::new(config_repo.clone(), repo.clone()));

        Harness {
            pool,
            repo,
            config_repo,
            manager,
            config_service,
        }
    }

    impl Harness {
        /// The state the credential handlers extract from `AppState`, with a refresh service that
        /// resolves and persists for real but takes its new credentials from `StubCredentialManager`
        /// instead of a platform API.
        fn route_state(&self, refreshed_to: &str, refresh_token: &str) -> CredentialRouteState {
            let mut credential_service = CredentialRefreshService::new(
                Arc::new(CredentialResolver::new(self.config_repo.clone())),
                Arc::new(SqlxCredentialStore::new(
                    self.pool.clone(),
                    self.pool.clone(),
                )),
            );
            credential_service.register_manager(Arc::new(StubCredentialManager::new(
                "bilibili",
                refreshed_to,
                refresh_token,
            )));

            CredentialRouteState {
                config_service: self.config_service.clone(),
                credential_service: Arc::new(credential_service),
                streamer_manager: self.manager.clone(),
            }
        }
    }

    fn saved_config(raw: Option<&str>) -> serde_json::Value {
        serde_json::from_str(raw.expect("streamer carries a config document"))
            .expect("config document is valid JSON")
    }

    /// Rename a streamer the way `PUT /api/streamers/{id}` does, rebuilding the whole row from
    /// the manager's metadata cache.
    async fn rename(manager: &CredentialStreamerManager, name: &str) {
        manager
            .partial_update_streamer(StreamerUpdateParams {
                id: STREAMER_ID.to_string(),
                name: Some(name.to_string()),
                url: None,
                platform_config_id: None,
                template_config_id: None,
                priority: None,
                state: None,
                streamer_specific_config: None,
            })
            .await
            .expect("rename succeeds");
    }

    /// `partial_update_streamer` rebuilds the whole streamers row from the manager's metadata
    /// cache, so an unrelated edit must not roll the row back to the pre-login credentials.
    #[tokio::test]
    async fn saved_credentials_survive_a_later_streamer_edit() {
        let h = harness("platform-bilibili", Some(r#"{"quality":"best"}"#)).await;

        save_streamer_credentials(
            &h.config_service,
            &h.manager,
            STREAMER_ID,
            "SESSDATA=abc",
            "refresh-1",
            Some("access-1"),
        )
        .await
        .expect("credentials are saved");

        // The manager's cache answers `StreamerManager::get_streamer` for the streamer routes and
        // the scheduler, so it has to carry the credentials as soon as the save returns.
        let cached = saved_config(
            h.manager
                .get_streamer(STREAMER_ID)
                .expect("still hydrated")
                .streamer_specific_config
                .as_deref(),
        );
        assert_eq!(cached["cookies"], "SESSDATA=abc");

        rename(&h.manager, "Renamed").await;

        let row = h.repo.get_streamer(STREAMER_ID).await.expect("row exists");
        assert_eq!(row.name, "Renamed");

        let persisted = saved_config(row.streamer_specific_config.as_deref());
        assert_eq!(persisted["cookies"], "SESSDATA=abc");
        assert_eq!(persisted["refresh_token"], "refresh-1");
        assert_eq!(persisted["access_token"], "access-1");
        assert_eq!(persisted["quality"], "best");
    }

    /// The credentials come from a bilibili login, so a streamer on another platform config is
    /// refused and its row left alone.
    #[tokio::test]
    async fn saving_credentials_is_refused_for_a_non_bilibili_streamer() {
        let h = harness("platform-douyin", None).await;

        let error = save_streamer_credentials(
            &h.config_service,
            &h.manager,
            STREAMER_ID,
            "SESSDATA=abc",
            "refresh-1",
            None,
        )
        .await
        .expect_err("a douyin streamer is refused");
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("is not bilibili"),
            "unexpected error: {}",
            error.message
        );

        let row = h.repo.get_streamer(STREAMER_ID).await.expect("row exists");
        assert!(row.streamer_specific_config.is_none());
    }

    #[tokio::test]
    async fn saving_credentials_reports_an_unknown_streamer() {
        let h = harness("platform-bilibili", None).await;

        let error = save_streamer_credentials(
            &h.config_service,
            &h.manager,
            "no-such-streamer",
            "SESSDATA=abc",
            "refresh-1",
            None,
        )
        .await
        .expect_err("an unknown streamer is refused");
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    }

    /// `refresh_streamer_credentials` persists through `CredentialStore::update_credentials`,
    /// which rewrites `streamer_specific_config` with its own SQL and leaves the manager's cache
    /// on the previous document, so the handler has to reload it before the next streamer edit
    /// rebuilds the row from that cache.
    #[tokio::test]
    async fn refreshed_credentials_survive_a_later_streamer_edit() {
        let h = harness(
            "platform-bilibili",
            Some(r#"{"cookies":"SESSDATA=old","refresh_token":"refresh-old"}"#),
        )
        .await;

        let response = refresh_streamer_credentials(
            State(h.route_state("SESSDATA=new", "refresh-new")),
            Path(STREAMER_ID.to_string()),
        )
        .await
        .expect("refresh succeeds");
        assert!(response.refreshed);

        rename(&h.manager, "Renamed").await;

        let persisted = saved_config(
            h.repo
                .get_streamer(STREAMER_ID)
                .await
                .expect("row exists")
                .streamer_specific_config
                .as_deref(),
        );
        assert_eq!(persisted["cookies"], "SESSDATA=new");
        assert_eq!(persisted["refresh_token"], "refresh-new");
    }

    #[test]
    fn merge_streamer_credentials_keeps_unrelated_keys() {
        let merged = merge_streamer_credentials(
            Some(r#"{"quality":"best","access_token":"stale"}"#),
            "SESSDATA=abc",
            "refresh-1",
            None,
        )
        .expect("merge succeeds");

        let config: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(config["quality"], "best");
        assert_eq!(config["cookies"], "SESSDATA=abc");
        assert_eq!(config["refresh_token"], "refresh-1");
        // A poll without an access token leaves the stored one alone.
        assert_eq!(config["access_token"], "stale");
    }

    #[test]
    fn merge_streamer_credentials_replaces_a_non_object_document() {
        let merged = merge_streamer_credentials(Some("[1, 2]"), "SESSDATA=abc", "refresh-1", None)
            .expect("merge succeeds");

        let config: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(config["cookies"], "SESSDATA=abc");
        assert_eq!(config.as_object().expect("object").len(), 2);
    }
}
