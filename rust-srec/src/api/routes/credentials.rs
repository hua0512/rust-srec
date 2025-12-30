//! Credential management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::credentials::platforms::bilibili::{BilibiliCredentialManager, QrPollStatus};
use crate::credentials::{CredentialScope, CredentialSource};

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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

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
    let source = CredentialSource::new(
        CredentialScope::Platform {
            platform_id: platform.id,
            platform_name: platform.platform_name.clone(),
        },
        cookies.to_string(),
        refresh_token,
        platform.platform_name,
    );

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
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TemplateCredentialQuery>,
) -> ApiResult<Json<CredentialSourceResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

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

    let source = CredentialSource::new(
        CredentialScope::Template {
            template_id: template.id,
            template_name: template.name,
        },
        cookies.to_string(),
        refresh_token,
        platform_name,
    );

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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
    let credential_service = state
        .credential_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Credential service not available"))?;

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
                CredentialScope::Streamer { .. } => config_service.invalidate_streamer(&id),
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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
    let credential_service = state
        .credential_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Credential service not available"))?;

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
    let source = CredentialSource::new(
        CredentialScope::Platform {
            platform_id: platform.id.clone(),
            platform_name: platform.platform_name.clone(),
        },
        cookies.to_string(),
        refresh_token,
        platform.platform_name,
    );

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
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TemplateCredentialQuery>,
) -> ApiResult<Json<CredentialRefreshResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
    let credential_service = state
        .credential_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Credential service not available"))?;

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

    let source = CredentialSource::new(
        CredentialScope::Template {
            template_id: template.id.clone(),
            template_name: template.name,
        },
        cookies.to_string(),
        refresh_token,
        platform_name,
    );

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
pub async fn bilibili_qr_generate(
    State(_state): State<AppState>,
) -> ApiResult<Json<QrGenerateApiResponse>> {
    let client = reqwest::Client::new();
    let manager = BilibiliCredentialManager::new(client)
        .map_err(|e| ApiError::internal(format!("Failed to create manager: {}", e)))?;

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
    State(state): State<AppState>,
    Json(body): Json<QrPollRequest>,
) -> ApiResult<Json<QrPollApiResponse>> {
    let client = reqwest::Client::new();
    let manager = BilibiliCredentialManager::new(client)
        .map_err(|e| ApiError::internal(format!("Failed to create manager: {}", e)))?;

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
        match &body.scope {
            CredentialSaveScope::Platform { id } => {
                let cs = state
                    .config_service
                    .as_ref()
                    .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
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
                let cs = state
                    .config_service
                    .as_ref()
                    .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
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
                let cs = state
                    .config_service
                    .as_ref()
                    .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;
                let streamer_repo = state.streamer_repository.as_ref().ok_or_else(|| {
                    ApiError::service_unavailable("Streamer repository not available")
                })?;

                let mut streamer = streamer_repo
                    .get_streamer(id)
                    .await
                    .map_err(ApiError::from)?;
                let platform = cs
                    .get_platform_config(&streamer.platform_config_id)
                    .await
                    .map_err(ApiError::from)?;

                if !platform.platform_name.eq_ignore_ascii_case("bilibili") {
                    return Err(ApiError::bad_request(format!(
                        "Streamer {id} is not bilibili"
                    )));
                }

                let mut config: serde_json::Value = streamer
                    .streamer_specific_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                if !config.is_object() {
                    config = serde_json::json!({});
                }
                if let Some(map) = config.as_object_mut() {
                    map.insert(
                        "cookies".to_string(),
                        serde_json::Value::String(cookies.clone()),
                    );
                    map.insert(
                        "refresh_token".to_string(),
                        serde_json::Value::String(refresh_token.clone()),
                    );
                }

                streamer.streamer_specific_config = serde_json::to_string(&config).ok();
                streamer_repo
                    .update_streamer(&streamer)
                    .await
                    .map_err(ApiError::from)?;
                cs.invalidate_streamer(id);
                tracing::info!(streamer_id = %id, "Saved QR credentials to streamer");

                saved_scope = Some(CredentialScope::Streamer {
                    streamer_id: id.clone(),
                    streamer_name: streamer.name.clone(),
                });
            }
        }
    }

    if let Some(scope) = &saved_scope
        && let Some(credential_service) = &state.credential_service
    {
        credential_service.invalidate(scope);
    }

    Ok(Json(QrPollApiResponse {
        status: status.to_string(),
        success: result.status == QrPollStatus::Success,
        message: message.to_string(),
    }))
}
