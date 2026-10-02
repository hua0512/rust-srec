//! Profile management exposes metadata; material is accepted only as input.

use axum::{
    Extension, Json, Router,
    extract::{FromRef, Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::{
    auth_service::AuthPrincipal,
    error::{ApiError, ApiResult},
    server::AppState,
};
use crate::credentials::conversion::{ConversionPreview, ConversionResult, ConvertLegacyRequest};
use crate::credentials::login_sessions::{
    CredentialLoginGenerated, CredentialLoginReceipt, CredentialLoginTarget,
};
use crate::credentials::{
    CredentialMaterial, CredentialOwner, CredentialProfileHealth, CredentialProfileSummary,
    OperationDeadline,
};
use crate::credentials::{CredentialSelection, ResolvedCredentialPolicy};

#[derive(Clone)]
pub struct CredentialProfileRouteState {
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::SqlxConfigRepository,
            crate::database::repositories::SqlxStreamerRepository,
        >,
    >,
    credential_profiles: std::sync::Arc<crate::database::repositories::CredentialProfileRepository>,
    credential_service: std::sync::Arc<crate::credentials::CredentialRefreshService>,
    credential_execution: std::sync::Arc<crate::credentials::CredentialExecutionService>,
    credential_login_sessions:
        std::sync::Arc<crate::credentials::login_sessions::CredentialLoginSessions>,
    credential_conversion:
        std::sync::Arc<crate::credentials::conversion::CredentialConversionService>,
    streamer_repository: std::sync::Arc<dyn crate::database::repositories::StreamerRepository>,
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
}

impl FromRef<AppState> for CredentialProfileRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
            credential_profiles: state.credential_profiles.clone(),
            credential_service: state.credential_service.clone(),
            credential_execution: state.credential_execution.clone(),
            credential_login_sessions: state.credential_login_sessions.clone(),
            credential_conversion: state.credential_conversion.clone(),
            streamer_repository: state.streamer_repository.clone(),
            auth_service: state.auth_service.clone(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ProfileScopeQuery {
    pub scope_type: String,
    pub scope_id: String,
    pub platform_id: String,
}

impl ProfileScopeQuery {
    fn owner(&self) -> ApiResult<CredentialOwner> {
        if self.scope_id.trim().is_empty() || self.platform_id.trim().is_empty() {
            return Err(ApiError::validation("scope and platform IDs are required"));
        }
        match self.scope_type.as_str() {
            "platform" => Ok(CredentialOwner::Platform {
                platform_id: self.scope_id.clone(),
            }),
            "template" => Ok(CredentialOwner::Template {
                template_id: self.scope_id.clone(),
            }),
            "streamer" => Ok(CredentialOwner::Streamer {
                streamer_id: self.scope_id.clone(),
            }),
            _ => Err(ApiError::validation("unknown credential owner type")),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProfileRequest {
    pub owner: CredentialOwner,
    pub platform_id: String,
    pub label: String,
    pub enabled: bool,
    pub material: CredentialMaterial,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileRequest {
    pub expected_version: i64,
    pub label: Option<String>,
    pub enabled: Option<bool>,
    pub replacement: Option<CredentialMaterial>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteProfileRequest {
    pub expected_version: i64,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileCapabilities {
    pub validate: bool,
    pub refresh: bool,
    pub qr_login: bool,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileDetail {
    pub profile: CredentialProfileSummary,
    pub health: Option<CredentialProfileHealth>,
    pub references: Vec<String>,
    pub capabilities: ProfileCapabilities,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveCredentialSelection {
    pub configured: Option<CredentialSelection>,
    pub resolved: Option<ResolvedCredentialPolicy>,
    pub candidates: Vec<ProfileDetail>,
    pub unavailable_reason: Option<String>,
    pub unavailable_retry_at: Option<i64>,
    pub active_binding: Option<crate::credentials::CredentialBinding>,
}

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    CredentialProfileRouteState: FromRef<S>,
{
    Router::new()
        .route("/selection", get(get_selection))
        .route("/convert-legacy/preview", get(preview_conversion))
        .route("/convert-legacy", post(convert_legacy))
        .route("/profiles", get(list_profiles).post(create_profile))
        .route(
            "/profiles/{id}",
            get(get_profile)
                .patch(update_profile)
                .delete(delete_profile),
        )
        .route("/profiles/{id}/validate", post(validate_profile))
        .route("/profiles/{id}/refresh", post(refresh_profile))
        .route("/login-sessions", post(generate_login))
        .route("/login-sessions/{id}/poll", post(poll_login))
}

#[utoipa::path(get, path = "/api/credentials/convert-legacy/preview", tag = "credentials", params(ProfileScopeQuery), responses((status = 200, body = ConversionPreview)), security(("bearer_auth" = [])))]
pub async fn preview_conversion(
    State(state): State<CredentialProfileRouteState>,
    Query(query): Query<ProfileScopeQuery>,
) -> ApiResult<Json<ConversionPreview>> {
    Ok(Json(
        state
            .credential_conversion
            .preview(query.owner()?, query.platform_id)
            .await
            .map_err(profile_error)?,
    ))
}

#[utoipa::path(post, path = "/api/credentials/convert-legacy", tag = "credentials", request_body = ConvertLegacyRequest, responses((status = 200, body = ConversionResult)), security(("bearer_auth" = [])))]
pub async fn convert_legacy(
    State(state): State<CredentialProfileRouteState>,
    Json(body): Json<ConvertLegacyRequest>,
) -> ApiResult<Json<ConversionResult>> {
    Ok(Json(
        state
            .credential_conversion
            .convert(body)
            .await
            .map_err(profile_error)?,
    ))
}

/// Kept as the route-level hook; codes come from the one global mapping.
pub(crate) fn profile_error(error: crate::Error) -> ApiError {
    ApiError::from(error)
}

async fn detail(state: &CredentialProfileRouteState, id: &str) -> ApiResult<ProfileDetail> {
    let record = state
        .credential_profiles
        .get(id)
        .await
        .map_err(profile_error)?;
    let platform = state
        .config_service
        .get_platform_config(&record.platform_config_id)
        .await
        .map_err(profile_error)?;
    let name = platform.platform_name.to_ascii_lowercase();
    let manager = state.credential_service.manager(&name);
    Ok(ProfileDetail {
        profile: record
            .summary()
            .map_err(|error| profile_error(error.into()))?,
        health: state
            .credential_profiles
            .health(id)
            .await
            .map_err(profile_error)?,
        references: state
            .credential_profiles
            .references(id)
            .await
            .map_err(profile_error)?,
        capabilities: ProfileCapabilities {
            validate: manager.is_some(),
            refresh: manager.is_some_and(|manager| manager.supports_auto_refresh()),
            qr_login: name == "bilibili",
        },
    })
}

#[utoipa::path(get, path = "/api/credentials/selection", tag = "credentials", params(ProfileScopeQuery), responses((status = 200, body = EffectiveCredentialSelection)), security(("bearer_auth" = [])))]
pub async fn get_selection(
    State(state): State<CredentialProfileRouteState>,
    Query(query): Query<ProfileScopeQuery>,
) -> ApiResult<Json<EffectiveCredentialSelection>> {
    let owner = query.owner()?;
    // Ownership validation also rejects retired targets, without selecting a candidate.
    state
        .credential_profiles
        .accessible(&owner, &query.platform_id)
        .await
        .map_err(profile_error)?;
    let platform = state
        .config_service
        .get_platform_config(&query.platform_id)
        .await
        .map_err(profile_error)?;
    let mut streamer = crate::domain::Streamer::new(
        "credential-status",
        crate::domain::StreamerUrl::from_trusted("https://example.invalid/credential-status"),
        &platform.id,
    );
    let template = match &owner {
        CredentialOwner::Template { template_id } => Some(
            state
                .config_service
                .get_template_config(template_id)
                .await
                .map_err(profile_error)?,
        ),
        CredentialOwner::Streamer { streamer_id } => {
            let record = state
                .streamer_repository
                .get_streamer(streamer_id)
                .await
                .map_err(profile_error)?;
            streamer.id = record.id;
            streamer.streamer_specific_config = record
                .streamer_specific_config
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(ApiError::from)?;
            match record.template_config_id {
                Some(id) => Some(
                    state
                        .config_service
                        .get_template_config(&id)
                        .await
                        .map_err(profile_error)?,
                ),
                None => None,
            }
        }
        CredentialOwner::Platform { .. } => None,
    };
    let configured = match &owner {
        CredentialOwner::Platform { .. } => platform
            .credential_selection
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(ApiError::from)?,
        CredentialOwner::Template { .. } => {
            let overrides = template
                .as_ref()
                .and_then(|template| template.platform_overrides.as_deref())
                .map(serde_json::from_str::<serde_json::Value>)
                .transpose()
                .map_err(ApiError::from)?;
            overrides
                .as_ref()
                .and_then(|value| value.get(&platform.platform_name))
                .and_then(|value| value.get("credential_selection"))
                .cloned()
        }
        CredentialOwner::Streamer { .. } => streamer
            .streamer_specific_config
            .as_ref()
            .and_then(|value| value.get("credential_selection"))
            .cloned(),
    }
    .map(CredentialSelection::from_value)
    .transpose()
    .map_err(profile_error)?;
    let authentication =
        crate::credentials::resolve_authentication(&streamer, &platform, template.as_ref())
            .map_err(profile_error)?;
    let mut candidates = Vec::new();
    if let Some(policy) = &authentication.policy {
        for id in policy.selection.profile_ids() {
            candidates.push(detail(&state, id).await?);
        }
    }
    let now = crate::database::time::now_ms();
    let mut exclusions = crate::credentials::Exclusions::default();
    let selectable = candidates
        .iter()
        .filter(|candidate| {
            exclusions.admits(candidate.profile.enabled, candidate.health.as_ref(), now)
        })
        .count();
    // The stored candidates outrank the last operation's outcome, which can
    // predate a re-enable or a new login.
    let current = (selectable == 0).then(|| exclusions.reason()).flatten();
    let active_binding = match &owner {
        CredentialOwner::Streamer { streamer_id } => state
            .credential_profiles
            .active_binding(streamer_id)
            .await
            .map_err(profile_error)?,
        _ => None,
    };
    let last_unavailable = authentication
        .policy
        .as_ref()
        .and_then(|policy| state.credential_execution.unavailable_status(policy));
    let (unavailable_reason, unavailable_retry_at) = match current {
        Some(reason) => (Some(reason.to_owned()), exclusions.retry_at()),
        None => last_unavailable
            .map(|status| (Some(status.reason), status.retry_at))
            .unwrap_or_default(),
    };
    Ok(Json(EffectiveCredentialSelection {
        configured,
        resolved: authentication.policy,
        candidates,
        unavailable_reason,
        unavailable_retry_at,
        active_binding,
    }))
}

#[utoipa::path(get, path = "/api/credentials/profiles", tag = "credentials", params(ProfileScopeQuery), responses((status = 200, body = Vec<ProfileDetail>)), security(("bearer_auth" = [])))]
pub async fn list_profiles(
    State(state): State<CredentialProfileRouteState>,
    Query(query): Query<ProfileScopeQuery>,
) -> ApiResult<Json<Vec<ProfileDetail>>> {
    let profiles = state
        .credential_profiles
        .accessible(&query.owner()?, &query.platform_id)
        .await
        .map_err(profile_error)?;
    let mut result = Vec::with_capacity(profiles.len());
    for profile in profiles {
        result.push(detail(&state, &profile.id).await?);
    }
    Ok(Json(result))
}

#[utoipa::path(get, path = "/api/credentials/profiles/{id}", tag = "credentials", params(("id" = String, Path)), responses((status = 200, body = ProfileDetail)), security(("bearer_auth" = [])))]
pub async fn get_profile(
    State(state): State<CredentialProfileRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProfileDetail>> {
    Ok(Json(detail(&state, &id).await?))
}

// Detached mutation tasks own the commit/publication boundary when HTTP callers disconnect.
async fn mutation<T: Send + 'static>(
    future: impl std::future::Future<Output = ApiResult<T>> + Send + 'static,
) -> ApiResult<T> {
    tokio::spawn(future)
        .await
        .map_err(|_| ApiError::internal("Credential mutation task failed"))?
}

#[utoipa::path(post, path = "/api/credentials/profiles", tag = "credentials", request_body = CreateProfileRequest, responses((status = 200, body = CredentialProfileSummary)), security(("bearer_auth" = [])))]
pub async fn create_profile(
    State(state): State<CredentialProfileRouteState>,
    Json(body): Json<CreateProfileRequest>,
) -> ApiResult<Json<CredentialProfileSummary>> {
    mutation(async move {
        let record = state
            .credential_profiles
            .create(
                &body.owner,
                &body.platform_id,
                &body.label,
                body.enabled,
                &body.material,
            )
            .await
            .map_err(profile_error)?;
        Ok(Json(
            record
                .summary()
                .map_err(|error| profile_error(error.into()))?,
        ))
    })
    .await
}

#[utoipa::path(patch, path = "/api/credentials/profiles/{id}", tag = "credentials", params(("id" = String, Path)), request_body = UpdateProfileRequest, responses((status = 200, body = CredentialProfileSummary)), security(("bearer_auth" = [])))]
pub async fn update_profile(
    State(state): State<CredentialProfileRouteState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> ApiResult<Json<CredentialProfileSummary>> {
    mutation(async move {
        let record = state
            .credential_profiles
            .update(
                &id,
                body.expected_version,
                body.label.as_deref(),
                body.enabled,
                body.replacement.as_ref(),
            )
            .await
            .map_err(profile_error)?;
        Ok(Json(
            record
                .summary()
                .map_err(|error| profile_error(error.into()))?,
        ))
    })
    .await
}

#[utoipa::path(delete, path = "/api/credentials/profiles/{id}", tag = "credentials", params(("id" = String, Path)), request_body = DeleteProfileRequest, responses((status = 200)), security(("bearer_auth" = [])))]
pub async fn delete_profile(
    State(state): State<CredentialProfileRouteState>,
    Path(id): Path<String>,
    Json(body): Json<DeleteProfileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    mutation(async move {
        state
            .credential_profiles
            .delete(&id, body.expected_version)
            .await
            .map_err(profile_error)?;
        Ok(Json(serde_json::json!({"deleted": true})))
    })
    .await
}

#[utoipa::path(post, path = "/api/credentials/profiles/{id}/validate", tag = "credentials", params(("id" = String, Path)), responses((status = 200, body = ProfileDetail)), security(("bearer_auth" = [])))]
pub async fn validate_profile(
    State(state): State<CredentialProfileRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProfileDetail>> {
    mutation(async move {
        state
            .credential_execution
            .validate_profile(&id, OperationDeadline::default())
            .await
            .map_err(profile_error)?;
        Ok(Json(detail(&state, &id).await?))
    })
    .await
}

#[utoipa::path(post, path = "/api/credentials/profiles/{id}/refresh", tag = "credentials", params(("id" = String, Path)), responses((status = 200, body = ProfileDetail)), security(("bearer_auth" = [])))]
pub async fn refresh_profile(
    State(state): State<CredentialProfileRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProfileDetail>> {
    mutation(async move {
        state
            .credential_execution
            .refresh_profile(&id, OperationDeadline::default())
            .await
            .map_err(profile_error)?;
        Ok(Json(detail(&state, &id).await?))
    })
    .await
}

fn principal(
    state: &CredentialProfileRouteState,
    principal: Option<Extension<AuthPrincipal>>,
) -> ApiResult<String> {
    match principal {
        Some(Extension(principal)) => Ok(principal.claims.sub),
        None if state.auth_service.is_none() => Ok("local-anonymous".into()),
        None => Err(ApiError::unauthorized("Authentication required")),
    }
}

fn private_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    headers
}

#[utoipa::path(post, path = "/api/credentials/login-sessions", tag = "credentials", request_body = CredentialLoginTarget, responses((status = 200, body = CredentialLoginGenerated)), security(("bearer_auth" = [])))]
pub async fn generate_login(
    State(state): State<CredentialProfileRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Json(target): Json<CredentialLoginTarget>,
) -> ApiResult<(HeaderMap, Json<CredentialLoginGenerated>)> {
    let principal = principal(&state, identity)?;
    let generated = state
        .credential_login_sessions
        .generate(&principal, target)
        .await
        .map_err(profile_error)?;
    Ok((private_headers(), Json(generated)))
}

#[utoipa::path(post, path = "/api/credentials/login-sessions/{id}/poll", tag = "credentials", params(("id" = String, Path)), responses((status = 200, body = CredentialLoginReceipt)), security(("bearer_auth" = [])))]
pub async fn poll_login(
    State(state): State<CredentialProfileRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> ApiResult<(HeaderMap, Json<CredentialLoginReceipt>)> {
    let principal = principal(&state, identity)?;
    mutation(async move {
        let receipt = state
            .credential_login_sessions
            .poll(&principal, &id)
            .await
            .map_err(profile_error)?;
        Ok((private_headers(), Json(receipt)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::api::auth_service::{AuthConfig, AuthService};
    use crate::database::models::{ApiKeyAccessLevel, UserDbModel};
    use crate::database::repositories::{
        CredentialProfileRepository, SqlxApiKeyRepository, SqlxConfigRepository,
        SqlxCredentialStore, SqlxRefreshTokenRepository, SqlxStreamerRepository,
        SqlxUserRepository, UserRepository,
    };

    use super::*;

    struct Fixture {
        pool: sqlx::SqlitePool,
        state: CredentialProfileRouteState,
        app: Router,
        full_key: String,
        read_key: String,
        user_id: String,
    }

    async fn fixture() -> Fixture {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let profiles = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let mut refresh = crate::credentials::CredentialRefreshService::new(Arc::new(
            SqlxCredentialStore::new(pool.clone(), pool.clone()),
        ));
        refresh.register_manager(Arc::new(
            crate::credentials::test_support::StubCredentialManager::new(
                "bilibili",
                "sid=refreshed-a",
                "new-token",
            ),
        ));
        let refresh = Arc::new(refresh);
        let execution = Arc::new(crate::credentials::CredentialExecutionService::new(
            profiles.clone(),
            refresh.clone(),
        ));
        let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let config = Arc::new(crate::config::ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            streamers.clone(),
        ));
        let writer = Arc::new(
            crate::database::CommittedWriter::new(
                pool.clone(),
                Arc::new(crate::utils::task_supervisor::TaskSupervisor::for_committed_work()),
            )
            .unwrap(),
        );
        let committed = Arc::new(crate::streamer::CommittedStreamerState::new(
            writer.clone(),
            crate::config::ConfigEventBroadcaster::new(),
        ));
        let conversion = Arc::new(
            crate::credentials::conversion::CredentialConversionService::new(
                pool.clone(),
                writer,
                profiles.clone(),
                committed,
            ),
        );
        let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
        let mut user = UserDbModel::new("credential-test-user", "unused-hash", vec!["user".into()]);
        user.must_change_password = false;
        users.create(&user).await.unwrap();
        let auth = Arc::new(AuthService::new(
            users,
            Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
            Arc::new(crate::api::jwt::JwtService::new(
                "test-credential-profile-key-32-chars",
                "test",
                "test",
                Some(3600),
            )),
            AuthConfig::default(),
        ));
        let (_, full_key) = auth
            .create_api_key(&user.id, "full", ApiKeyAccessLevel::Full, None)
            .await
            .unwrap();
        let (_, read_key) = auth
            .create_api_key(&user.id, "read", ApiKeyAccessLevel::ReadOnly, None)
            .await
            .unwrap();
        let state = CredentialProfileRouteState {
            config_service: config,
            credential_profiles: profiles.clone(),
            credential_service: refresh.clone(),
            credential_execution: execution,
            credential_login_sessions: Arc::new(
                crate::credentials::login_sessions::CredentialLoginSessions::new(
                    profiles,
                    pool.clone(),
                    pool.clone(),
                    refresh.admission(),
                ),
            ),
            credential_conversion: conversion,
            streamer_repository: streamers,
            auth_service: Some(auth.clone()),
        };
        let app = Router::new()
            .nest("/api/credentials", router())
            .layer(crate::api::middleware::AuthLayer::new(auth))
            .with_state(state.clone());
        Fixture {
            pool,
            state,
            app,
            full_key,
            read_key,
            user_id: user.id,
        }
    }

    async fn request(
        app: &Router,
        method: &str,
        path: &str,
        key: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(key) = key {
            request = request.header("authorization", format!("Bearer {key}"));
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn create_body(label: &str) -> serde_json::Value {
        serde_json::json!({"owner":{"type":"platform","platform_id":"platform-bilibili"},"platform_id":"platform-bilibili","label":label,"enabled":true,"material":{"cookies":"sid=secret-sentinel","refresh_token":"token-sentinel"}})
    }

    #[tokio::test]
    async fn management_requires_auth_and_full_access_keys() {
        let fixture = fixture().await;
        let path = "/api/credentials/profiles?scope_type=platform&scope_id=platform-bilibili&platform_id=platform-bilibili";
        assert_eq!(
            request(&fixture.app, "GET", path, None, serde_json::Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &fixture.app,
                "GET",
                path,
                Some(&fixture.read_key),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &fixture.app,
                "GET",
                path,
                Some(&fixture.full_key),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                "/api/credentials/profiles",
                Some(&fixture.read_key),
                create_body("A")
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn summaries_never_return_secrets_and_stale_edits_conflict() {
        let fixture = fixture().await;
        let (status, profile) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            create_body("A"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!profile.to_string().contains("sentinel"));
        let path = format!(
            "/api/credentials/profiles/{}",
            profile["id"].as_str().unwrap()
        );
        let edit = serde_json::json!({"expected_version":profile["version"],"label":"Renamed"});
        let (status, updated) = request(
            &fixture.app,
            "PATCH",
            &path,
            Some(&fixture.full_key),
            edit.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["revision"], profile["revision"]);
        let (status, error) =
            request(&fixture.app, "PATCH", &path, Some(&fixture.full_key), edit).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["code"], "CREDENTIAL_STALE_VERSION");
        let (_, detail) = request(
            &fixture.app,
            "GET",
            &path,
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert!(!detail.to_string().contains("sentinel"));
    }

    #[tokio::test]
    async fn targeted_refresh_does_not_mutate_another_account() {
        let fixture = fixture().await;
        let (_, a) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            create_body("A"),
        )
        .await;
        let (_, b) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            create_body("B"),
        )
        .await;
        let id = a["id"].as_str().unwrap();
        let (status, _) = request(
            &fixture.app,
            "POST",
            &format!("/api/credentials/profiles/{id}/refresh"),
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            fixture
                .state
                .credential_profiles
                .get(id)
                .await
                .unwrap()
                .cookies,
            "sid=refreshed-a"
        );
        assert_eq!(
            fixture
                .state
                .credential_profiles
                .get(b["id"].as_str().unwrap())
                .await
                .unwrap()
                .cookies,
            "sid=secret-sentinel"
        );
    }

    #[tokio::test]
    async fn unsupported_validation_reports_capability_without_marking_invalid() {
        let fixture = fixture().await;
        let mut body = create_body("Unvalidated");
        body["platform_id"] = "platform-twitch".into();
        body["owner"]["platform_id"] = "platform-twitch".into();
        let (_, profile) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            body,
        )
        .await;
        let path = format!(
            "/api/credentials/profiles/{}/validate",
            profile["id"].as_str().unwrap()
        );
        let (status, detail) = request(
            &fixture.app,
            "POST",
            &path,
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["capabilities"]["validate"], false);
        assert!(detail["health"].is_null());
        assert_eq!(detail["profile"]["revision"], profile["revision"]);
    }

    #[tokio::test]
    async fn qr_receipts_require_the_initiating_principal_and_contain_no_provider_material() {
        let fixture = fixture().await;
        let now = crate::database::time::now_ms();
        sqlx::query("INSERT INTO credential_login_sessions(id,principal,platform_config_id,target,provider_auth_code,created_at,expires_at,state,result_profile_id,result_version,completed_at) VALUES ('receipt',?,'platform-bilibili','{}','',?,?,'completed','profile-result',2,?)")
            .bind(&fixture.user_id).bind(now).bind(now + 600000).bind(now).execute(&fixture.pool).await.unwrap();
        let (status, receipt) = request(
            &fixture.app,
            "POST",
            "/api/credentials/login-sessions/receipt/poll",
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(receipt["profile_id"], "profile-result");
        assert!(receipt.get("provider_auth_code").is_none());
        sqlx::query(
            "UPDATE credential_login_sessions SET principal='another-user' WHERE id='receipt'",
        )
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                "/api/credentials/login-sessions/receipt/poll",
                Some(&fixture.full_key),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}
