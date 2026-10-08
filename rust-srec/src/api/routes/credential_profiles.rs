//! Profile management exposes metadata; material is accepted only as input.

use std::collections::HashMap;

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
use crate::credentials::login_sessions::{
    CredentialLoginGenerated, CredentialLoginReceipt, CredentialLoginTarget,
};
use crate::credentials::{
    CredentialAttention, CredentialMaterial, CredentialOwner, CredentialProfile,
    CredentialProfileHealth, CredentialProfileSummary, OperationDeadline, ProfileReferences,
    ProviderCapabilities,
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
    credential_service: std::sync::Arc<crate::credentials::CredentialProviderRegistry>,
    credential_execution: std::sync::Arc<crate::credentials::CredentialExecutionService>,
    credential_login_sessions:
        std::sync::Arc<crate::credentials::login_sessions::CredentialLoginSessions>,
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
            auth_service: state.auth_service.clone(),
        }
    }
}

/// A scope's selection on one platform. A template selects per platform, so
/// the platform is named even where the scope implies it.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ProfileScopeQuery {
    pub scope_type: String,
    pub scope_id: String,
    pub platform_id: String,
}

/// Accounts belong to a platform, so every scope that selects them reads the
/// same list.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct PlatformProfilesQuery {
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
    pub platform_id: String,
    pub label: String,
    pub enabled: bool,
    pub material: CredentialMaterial,
    /// The account's own route, used by every request made with it;
    /// `inherit`, the default, follows the recording using the account.
    #[serde(default)]
    pub proxy_route: crate::proxies::ProxyRoute,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileRequest {
    pub expected_version: i64,
    pub label: Option<String>,
    pub enabled: Option<bool>,
    pub replacement: Option<CredentialMaterial>,
    /// Replaces the account's own route; omitting it keeps it.
    #[serde(default)]
    pub proxy_route: Option<crate::proxies::ProxyRoute>,
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
    /// The selections that list the profile and the live recordings bound to it.
    pub references: ProfileReferences,
    pub capabilities: ProfileCapabilities,
    /// When the account is next renewed before use, in epoch milliseconds,
    /// for platforms that renew accounts by age. A time in the past means the
    /// next operation using it renews it. Absent when it is never renewed.
    pub next_renewal_at: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveCredentialSelection {
    pub configured: Option<CredentialSelection>,
    pub resolved: Option<ResolvedCredentialPolicy>,
    pub candidates: Vec<ProfileDetail>,
    pub unavailable_reason: Option<crate::credentials::UnavailableReason>,
    pub active_binding: Option<crate::credentials::CredentialBinding>,
}

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    CredentialProfileRouteState: FromRef<S>,
{
    Router::new()
        .route("/selection", get(get_selection))
        .route("/capabilities", get(get_capabilities))
        .route("/attention", get(list_attention))
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

async fn detail(state: &CredentialProfileRouteState, id: &str) -> ApiResult<ProfileDetail> {
    let record = state.credential_profiles.get(id).await?;
    let mut details = details(state, vec![record]).await?;
    details
        .pop()
        .ok_or_else(|| ApiError::internal("Credential profile detail missing"))
}

/// Details of several profiles, with health and references read once for
/// all of them rather than per profile.
async fn details(
    state: &CredentialProfileRouteState,
    records: Vec<CredentialProfile>,
) -> ApiResult<Vec<ProfileDetail>> {
    let ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    let mut health = state.credential_profiles.health_of(&ids).await?;
    let mut references = state.credential_profiles.references_of(&ids).await?;
    let mut platforms: HashMap<String, String> = HashMap::new();
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        let platform = match platforms.get(&record.platform_config_id) {
            Some(name) => name.clone(),
            None => {
                let name = state
                    .config_service
                    .get_platform_config(&record.platform_config_id)
                    .await?
                    .platform_name;
                platforms.insert(record.platform_config_id.clone(), name.clone());
                name
            }
        };
        let provider = state.credential_service.provider(&platform);
        let capabilities = provider.capabilities();
        let health = health.remove(&record.id);
        result.push(ProfileDetail {
            next_renewal_at: crate::credentials::next_renewal_at(
                provider,
                &record,
                health.as_ref(),
            )?,
            references: references.remove(&record.id).unwrap_or_default(),
            capabilities: ProfileCapabilities {
                validate: capabilities.check,
                refresh: capabilities.refresh && provider.refreshable(&record.material()?),
                qr_login: capabilities.qr_login,
            },
            profile: record.summary(),
            health,
        });
    }
    Ok(result)
}

/// What accounts on a platform accept and support, before any profile exists.
#[derive(Debug, Serialize, ToSchema)]
pub struct PlatformCredentialCapabilities {
    #[serde(flatten)]
    pub provider: ProviderCapabilities,
    /// Accounts are chosen per streamer, as none or one fixed account.
    pub per_streamer_selection: bool,
}

#[utoipa::path(get, path = "/api/credentials/capabilities", tag = "credentials", params(PlatformProfilesQuery), responses((status = 200, body = PlatformCredentialCapabilities)), security(("bearer_auth" = [])))]
pub async fn get_capabilities(
    State(state): State<CredentialProfileRouteState>,
    Query(query): Query<PlatformProfilesQuery>,
) -> ApiResult<Json<PlatformCredentialCapabilities>> {
    if query.platform_id.trim().is_empty() {
        return Err(ApiError::validation("platform ID is required"));
    }
    let platform = state
        .config_service
        .get_platform_config(&query.platform_id)
        .await?;
    Ok(Json(PlatformCredentialCapabilities {
        provider: crate::credentials::provider(&platform.platform_name).capabilities(),
        per_streamer_selection: crate::domain::is_streamlink_platform(&platform.platform_name),
    }))
}

/// Enabled accounts on every platform that need a new login, or whose
/// automatic refresh keeps failing after the platform rejected them. Disabled
/// accounts are a user choice and are not listed.
#[utoipa::path(get, path = "/api/credentials/attention", tag = "credentials", responses((status = 200, body = Vec<CredentialAttention>)), security(("bearer_auth" = [])))]
pub async fn list_attention(
    State(state): State<CredentialProfileRouteState>,
) -> ApiResult<Json<Vec<CredentialAttention>>> {
    Ok(Json(state.credential_profiles.needing_attention().await?))
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
        .await?;
    let layers = state
        .credential_profiles
        .selection_layers(&owner, &query.platform_id)
        .await?;
    let configured = layers
        .iter()
        .find(|stored| stored.owner == owner && stored.platform_id == query.platform_id)
        .map(|stored| stored.selection.clone());
    let resolved = crate::credentials::resolve_authentication(&query.platform_id, &layers)?;
    let mut records = Vec::new();
    if let Some(policy) = &resolved {
        for id in policy.selection.profile_ids() {
            records.push(state.credential_profiles.get(id).await?);
        }
    }
    let candidates = details(&state, records).await?;
    let mut exclusions = crate::credentials::Exclusions::default();
    let selectable = candidates
        .iter()
        .filter(|candidate| exclusions.admits(candidate.profile.enabled, candidate.health.as_ref()))
        .count();
    // The stored candidates outrank the last operation's outcome, which can
    // predate a re-enable or a new login.
    let current = (selectable == 0).then(|| exclusions.reason()).flatten();
    let active_binding = match &owner {
        CredentialOwner::Streamer { streamer_id } => {
            state
                .credential_profiles
                .active_binding(streamer_id)
                .await?
        }
        _ => None,
    };
    let last_unavailable = resolved
        .as_ref()
        .and_then(|policy| state.credential_execution.unavailable_status(policy));
    let unavailable_reason = current.or(last_unavailable.map(|status| status.reason));
    Ok(Json(EffectiveCredentialSelection {
        configured,
        resolved,
        candidates,
        unavailable_reason,
        active_binding,
    }))
}

#[utoipa::path(get, path = "/api/credentials/profiles", tag = "credentials", params(PlatformProfilesQuery), responses((status = 200, body = Vec<ProfileDetail>)), security(("bearer_auth" = [])))]
pub async fn list_profiles(
    State(state): State<CredentialProfileRouteState>,
    Query(query): Query<PlatformProfilesQuery>,
) -> ApiResult<Json<Vec<ProfileDetail>>> {
    if query.platform_id.trim().is_empty() {
        return Err(ApiError::validation("platform ID is required"));
    }
    let owner = CredentialOwner::Platform {
        platform_id: query.platform_id.clone(),
    };
    let profiles = state
        .credential_profiles
        .accessible(&owner, &query.platform_id)
        .await?;
    Ok(Json(details(&state, profiles).await?))
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
                &body.platform_id,
                &body.label,
                body.enabled,
                &body.material,
                &body.proxy_route,
            )
            .await?;
        Ok(Json(record.summary()))
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
                body.proxy_route.as_ref(),
            )
            .await?;
        Ok(Json(record.summary()))
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
            .await?;
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
            .await?;
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
            .await?;
        Ok(Json(detail(&state, &id).await?))
    })
    .await
}

#[utoipa::path(post, path = "/api/credentials/login-sessions", tag = "credentials", request_body = CredentialLoginTarget, responses((status = 200, body = CredentialLoginGenerated)), security(("bearer_auth" = [])))]
pub async fn generate_login(
    State(state): State<CredentialProfileRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Json(target): Json<CredentialLoginTarget>,
) -> ApiResult<(HeaderMap, Json<CredentialLoginGenerated>)> {
    let principal = super::request_principal(state.auth_service.is_some(), identity)?;
    let generated = state
        .credential_login_sessions
        .generate(&principal, target)
        .await?;
    Ok((super::private_response_headers(), Json(generated)))
}

#[utoipa::path(post, path = "/api/credentials/login-sessions/{id}/poll", tag = "credentials", params(("id" = String, Path)), responses((status = 200, body = CredentialLoginReceipt)), security(("bearer_auth" = [])))]
pub async fn poll_login(
    State(state): State<CredentialProfileRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> ApiResult<(HeaderMap, Json<CredentialLoginReceipt>)> {
    let principal = super::request_principal(state.auth_service.is_some(), identity)?;
    mutation(async move {
        let receipt = state
            .credential_login_sessions
            .poll(&principal, &id)
            .await?;
        Ok((super::private_response_headers(), Json(receipt)))
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
        ConfigRepository, CredentialProfileRepository, SqlxApiKeyRepository, SqlxConfigRepository,
        SqlxRefreshTokenRepository, SqlxStreamerRepository, SqlxUserRepository, UserRepository,
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
        let mut refresh = crate::credentials::CredentialProviderRegistry::new();
        refresh.register_provider(
            "bilibili",
            Arc::new(
                crate::credentials::test_support::StubCredentialProvider::new(
                    "sid=refreshed-a",
                    "new-token",
                ),
            ),
        );
        refresh.register_provider(
            "douyu",
            Arc::new(crate::credentials::platforms::DouyuProvider),
        );
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
        serde_json::json!({"platform_id":"platform-bilibili","label":label,"enabled":true,"material":{"cookies":"sid=secret-sentinel","refresh_token":"token-sentinel"}})
    }

    #[tokio::test]
    async fn management_requires_auth_and_full_access_keys() {
        let fixture = fixture().await;
        let path = "/api/credentials/profiles?platform_id=platform-bilibili";
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
    async fn profiles_are_listed_per_platform_without_a_selecting_scope() {
        let fixture = fixture().await;
        let (status, created) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            create_body("A"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let get = |path: &'static str| {
            let app = fixture.app.clone();
            let key = fixture.full_key.clone();
            async move { request(&app, "GET", path, Some(&key), serde_json::Value::Null).await }
        };
        let (status, listed) = get("/api/credentials/profiles?platform_id=platform-bilibili").await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<_> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|detail| detail["profile"]["id"].clone())
            .collect();
        assert_eq!(ids, vec![created["id"].clone()]);
        let (status, listed) = get("/api/credentials/profiles?platform_id=platform-twitch").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed, serde_json::json!([]));
        assert_eq!(
            get("/api/credentials/profiles?platform_id=platform-missing")
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get("/api/credentials/profiles?platform_id=%20").await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            get("/api/credentials/profiles?scope_type=platform&scope_id=platform-bilibili&platform_id=platform-bilibili")
                .await
                .0,
            StatusCode::BAD_REQUEST
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
    async fn an_account_route_names_a_saved_proxy_and_omitting_it_keeps_it() {
        let fixture = fixture().await;
        let saved = crate::database::repositories::proxies::save_for_test(
            &fixture.pool,
            "account proxy",
            "socks5h://proxy.example:1080",
        )
        .await;
        let mut body = create_body("Proxied");
        body["proxy_route"] = serde_json::json!({"kind": "proxy", "id": saved});
        let (status, profile) = request(
            &fixture.app,
            "POST",
            "/api/credentials/profiles",
            Some(&fixture.full_key),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{profile}");
        assert_eq!(
            profile["proxy_route"],
            serde_json::json!({"kind": "proxy", "id": saved})
        );
        let path = format!(
            "/api/credentials/profiles/{}",
            profile["id"].as_str().unwrap()
        );
        let (status, renamed) = request(
            &fixture.app,
            "PATCH",
            &path,
            Some(&fixture.full_key),
            serde_json::json!({"expected_version": profile["version"], "label": "Renamed"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(renamed["proxy_route"], profile["proxy_route"]);
        assert_eq!(renamed["revision"], profile["revision"]);
        let (status, direct) = request(
            &fixture.app,
            "PATCH",
            &path,
            Some(&fixture.full_key),
            serde_json::json!({
                "expected_version": renamed["version"],
                "proxy_route": {"kind": "direct"},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(direct["proxy_route"], serde_json::json!({"kind": "direct"}));
        assert_ne!(direct["revision"], renamed["revision"]);
        let (status, error) = request(
            &fixture.app,
            "PATCH",
            &path,
            Some(&fixture.full_key),
            serde_json::json!({
                "expected_version": direct["version"],
                "proxy_route": {"kind": "proxy", "id": "missing"},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
        assert_eq!(error["code"], "PROXY_NOT_FOUND");
        // The replaced inline proxy is not accepted.
        let (status, _) = request(
            &fixture.app,
            "PATCH",
            &path,
            Some(&fixture.full_key),
            serde_json::json!({
                "expected_version": direct["version"],
                "proxy": {"url": "http://proxy.example:8080"},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
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
    async fn platform_capabilities_describe_account_material_before_any_profile() {
        let fixture = fixture().await;
        let capabilities = |platform: &'static str| {
            let app = fixture.app.clone();
            let key = fixture.full_key.clone();
            async move {
                let (status, body) = request(
                    &app,
                    "GET",
                    &format!("/api/credentials/capabilities?platform_id={platform}"),
                    Some(&key),
                    serde_json::Value::Null,
                )
                .await;
                assert_eq!(status, StatusCode::OK);
                body
            }
        };
        let twitch = capabilities("platform-twitch").await;
        assert_eq!(twitch["token_only"], true);
        assert_eq!(twitch["check"], true);
        assert_eq!(twitch["refresh"], false);
        assert_eq!(twitch["per_streamer_selection"], false);
        let soop = capabilities("platform-soop").await;
        assert_eq!(soop["reauth_login"], true);
        let huya = capabilities("platform-huya").await;
        assert_eq!(huya["check"], false);
        assert_eq!(huya["access_token"], false);
        let streamlink = capabilities("platform-streamlink").await;
        assert_eq!(streamlink["per_streamer_selection"], true);
    }

    #[tokio::test]
    async fn douyu_profiles_offer_refresh_only_with_a_passport_credential() {
        let fixture = fixture().await;
        let (status, platform) = request(
            &fixture.app,
            "GET",
            "/api/credentials/capabilities?platform_id=platform-douyu",
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(platform["qr_login"], true);
        assert_eq!(platform["refresh"], true);
        assert_eq!(platform["refresh_token"], true);
        assert_eq!(platform["check"], false);
        for (material, refreshable) in [
            (
                serde_json::json!({"cookies":"acf_did=0123456789abcdef0123456789abcdef"}),
                false,
            ),
            (
                serde_json::json!({"cookies":"dy_did=bdevice; acf_did=bdevice; acf_uid=1; acf_auth=a","refresh_token":"passport-sentinel"}),
                true,
            ),
        ] {
            let (status, profile) = request(
                &fixture.app,
                "POST",
                "/api/credentials/profiles",
                Some(&fixture.full_key),
                serde_json::json!({"platform_id":"platform-douyu","label":"Douyu","enabled":true,"material":material}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{profile}");
            let (_, detail) = request(
                &fixture.app,
                "GET",
                &format!(
                    "/api/credentials/profiles/{}",
                    profile["id"].as_str().unwrap()
                ),
                Some(&fixture.full_key),
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(detail["capabilities"]["refresh"], refreshable);
            assert_eq!(detail["capabilities"]["qr_login"], true);
            assert_eq!(detail["capabilities"]["validate"], false);
            assert!(!detail.to_string().contains("passport-sentinel"));
            // Renewal counts from the profile's last change until a refresh.
            let updated_at: i64 =
                sqlx::query_scalar("SELECT updated_at FROM credential_profiles WHERE id = ?")
                    .bind(profile["id"].as_str().unwrap())
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap();
            let expected = refreshable.then_some(updated_at + 4 * 24 * 60 * 60 * 1000);
            assert_eq!(detail["next_renewal_at"], serde_json::json!(expected));
        }
    }

    #[tokio::test]
    async fn attention_lists_only_enabled_accounts_needing_the_user_without_secrets() {
        use crate::credentials::{CredentialValidity, HealthReason};

        let fixture = fixture().await;
        let profiles = fixture.state.credential_profiles.clone();
        let material = CredentialMaterial {
            cookies: "sid=secret-sentinel".into(),
            refresh_token: Some("token-sentinel".into()),
            access_token: None,
            reauth_config: None,
        };
        let create = |platform: &'static str, label: &'static str, enabled: bool| {
            let profiles = profiles.clone();
            let material = material.clone();
            async move {
                profiles
                    .create(
                        platform,
                        label,
                        enabled,
                        &material,
                        &crate::proxies::ProxyRoute::Inherit,
                    )
                    .await
                    .unwrap()
            }
        };
        let needs_refresh = |profile: CredentialProfile, failures: usize| {
            let profiles = profiles.clone();
            async move {
                profiles
                    .publish_health(
                        &profile,
                        CredentialValidity::NeedsRefresh,
                        Some(HealthReason::AuthenticationFailed),
                        true,
                    )
                    .await
                    .unwrap();
                for _ in 0..failures {
                    profiles.record_refresh_failure(&profile).await.unwrap();
                }
            }
        };

        let mut changes = profiles.attention_changes();
        let twitch_login = create("platform-twitch", "Twitch login", true).await;
        assert!(
            changes.has_changed().unwrap(),
            "creating an account wakes clients"
        );
        changes.mark_unchanged();
        profiles.mark_invalid(&twitch_login).await.unwrap();
        assert!(
            changes.has_changed().unwrap(),
            "a new invalid account wakes clients"
        );
        let login = create("platform-bilibili", "Needs login", true).await;
        profiles.mark_invalid(&login).await.unwrap();
        let disabled = create("platform-bilibili", "Disabled", true).await;
        profiles.mark_invalid(&disabled).await.unwrap();
        // Disabling through the repository also moves the revision past the
        // health; the listing leaves disabled accounts out even without that.
        sqlx::query("UPDATE credential_profiles SET enabled = 0 WHERE id = ?")
            .bind(&disabled.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let failing = create("platform-bilibili", "Refresh failing", true).await;
        needs_refresh(failing.clone(), 3).await;
        let retrying = create("platform-bilibili", "Refresh retrying", true).await;
        needs_refresh(retrying, 2).await;
        let valid = create("platform-bilibili", "Valid", true).await;
        profiles
            .publish_health(&valid, CredentialValidity::Valid, None, true)
            .await
            .unwrap();
        create("platform-bilibili", "Unchecked", true).await;

        let (status, listed) = request(
            &fixture.app,
            "GET",
            "/api/credentials/attention",
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary: Vec<_> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["profile"]["id"].as_str().unwrap().to_owned(),
                    entry["platform_name"].as_str().unwrap().to_owned(),
                    entry["reason"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (login.id.clone(), "bilibili".into(), "login_required".into()),
                (
                    failing.id.clone(),
                    "bilibili".into(),
                    "refresh_failing".into()
                ),
                (
                    twitch_login.id.clone(),
                    "twitch".into(),
                    "login_required".into()
                ),
            ]
        );
        assert_eq!(listed[1]["health"]["refresh_failure_count"], 3);
        assert_eq!(listed[1]["health"]["validity"], "needs_refresh");
        assert!(!listed.to_string().contains("sentinel"));

        // A new login replaces the material, which discards the stale health.
        profiles
            .update(
                &login.id,
                login.version,
                None,
                None,
                Some(&CredentialMaterial {
                    cookies: "sid=fresh-sentinel".into(),
                    ..material.clone()
                }),
                None,
            )
            .await
            .unwrap();
        let (_, listed) = request(
            &fixture.app,
            "GET",
            "/api/credentials/attention",
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(listed.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_account_list_names_its_users_and_a_delete_conflict_lists_them() {
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
        let id = profile["id"].as_str().unwrap().to_owned();
        assert_eq!(profile["last_used_at"], serde_json::Value::Null);
        let selection = CredentialSelection::Fixed {
            credential_id: id.clone(),
        };
        let platforms = SqlxConfigRepository::new(fixture.pool.clone(), fixture.pool.clone());
        let platform = platforms
            .get_platform_config("platform-bilibili")
            .await
            .unwrap();
        platforms
            .update_platform_config_with_selection(&platform, Some(&selection))
            .await
            .unwrap();
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        let binding = crate::credentials::CredentialBinding {
            identity: crate::credentials::CredentialIdentity::Profile {
                profile_id: id.clone(),
            },
            revision: 1,
            policy: ResolvedCredentialPolicy::new("platform-bilibili".into(), owner, selection)
                .unwrap(),
            epoch: 1,
        };
        sqlx::query("INSERT INTO live_sessions(id, streamer_name, start_time, credential_binding) VALUES ('live', 'Recorder', 1, ?)")
            .bind(serde_json::to_string(&binding).unwrap())
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE credential_profiles SET last_used_at = 1234 WHERE id = ?")
            .bind(&id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let expected = serde_json::json!({
            "selections": [{
                "owner": {"type": "platform", "platform_id": "platform-bilibili"},
                "name": "bilibili",
                "platform_id": "platform-bilibili",
                "platform_name": "bilibili",
            }],
            "recordings": [{
                "session_id": "live",
                "streamer_id": null,
                "streamer_name": "Recorder",
            }],
        });
        let (status, listed) = request(
            &fixture.app,
            "GET",
            "/api/credentials/profiles?platform_id=platform-bilibili",
            Some(&fixture.full_key),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed[0]["references"], expected);
        assert_eq!(listed[0]["profile"]["last_used_at"], 1234);
        // Bilibili accounts are not renewed by age.
        assert_eq!(listed[0]["next_renewal_at"], serde_json::Value::Null);

        let (status, error) = request(
            &fixture.app,
            "DELETE",
            &format!("/api/credentials/profiles/{id}"),
            Some(&fixture.full_key),
            serde_json::json!({"expected_version": profile["version"]}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["code"], "CREDENTIAL_PROFILE_REFERENCED");
        assert_eq!(error["details"]["references"], expected);
    }

    #[tokio::test]
    async fn unsupported_validation_reports_capability_without_marking_invalid() {
        let fixture = fixture().await;
        let mut body = create_body("Unvalidated");
        body["platform_id"] = "platform-huya".into();
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
