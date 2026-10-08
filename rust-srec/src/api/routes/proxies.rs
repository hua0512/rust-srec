//! Saved proxies. Responses show a proxy's username but never its password.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::repositories::proxies::ProxyUpdate;
use crate::proxies::{
    ProbeOutcome, ProxyEndpoint, ProxyEntry, ProxyName, ProxyReferences, ProxyService, RouteKind,
    RouteScope, RouteSource, SystemProxy, SystemProxySummary,
};

#[derive(Clone)]
pub struct ProxyRouteState {
    proxies: Arc<ProxyService>,
    config_service: Arc<
        crate::config::ConfigService<
            crate::database::repositories::SqlxConfigRepository,
            crate::database::repositories::SqlxStreamerRepository,
        >,
    >,
}

impl FromRef<AppState> for ProxyRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            proxies: state.proxies.clone(),
            config_service: state.config_service.clone(),
        }
    }
}

/// A saved proxy as the API shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyResponse {
    pub id: String,
    pub name: String,
    /// Canonical `scheme://host[:port]`.
    pub url: String,
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    /// A password is saved; it is never returned.
    pub has_password: bool,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// How many global, platform, template, streamer and account routes
    /// name the proxy.
    pub usage_count: usize,
}

impl ProxyResponse {
    fn new(entry: ProxyEntry, usage_count: usize) -> Self {
        let parsed = url::Url::parse(&entry.url).ok();
        Self {
            scheme: parsed
                .as_ref()
                .map(|url| url.scheme().to_owned())
                .unwrap_or_default(),
            host: parsed
                .as_ref()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_default(),
            port: parsed.as_ref().and_then(url::Url::port_or_known_default),
            has_password: entry.password.is_some(),
            id: entry.id,
            name: entry.name,
            url: entry.url,
            username: entry.username,
            version: entry.version,
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            usage_count,
        }
    }
}

/// A saved proxy with the routes naming it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyDetailResponse {
    pub proxy: ProxyResponse,
    pub references: ProxyReferences,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyRequest {
    pub name: String,
    /// `http`, `https`, `socks5` or `socks5h` URL without a login.
    pub url: String,
    /// A login needs both a username and a password.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProxyRequest {
    pub expected_version: i64,
    pub name: Option<String>,
    pub url: Option<String>,
    /// A new username; `null` removes the login, password included, and
    /// omitting it keeps the saved one.
    #[serde(
        default,
        deserialize_with = "crate::utils::json::deserialize_field_present_nullable"
    )]
    #[schema(value_type = Option<String>)]
    pub username: Option<Option<String>>,
    /// A new password; omitting it keeps the saved one.
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct DeleteProxyQuery {
    /// Refuses the delete when the proxy changed since this version was read.
    pub expected_version: Option<i64>,
}

/// What a check connects through and what it requests.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestProxyRequest {
    /// A saved proxy. With `url`, `username` or `password`, the saved proxy
    /// as edited; an omitted password keeps the saved one.
    #[serde(default)]
    pub proxy_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::utils::json::deserialize_field_present_nullable"
    )]
    #[schema(value_type = Option<String>)]
    pub username: Option<Option<String>>,
    #[serde(default)]
    pub password: Option<String>,
    /// Request this platform's home page.
    #[serde(default)]
    pub platform: Option<String>,
    /// Request this `http`/`https` URL instead.
    #[serde(default)]
    pub target_url: Option<String>,
}

impl std::fmt::Debug for TestProxyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestProxyRequest")
            .field("proxy_id", &self.proxy_id)
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

/// The scope whose effective route to report.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct EffectiveRouteQuery {
    /// `global`, `platform`, `template`, `streamer` or `account`.
    pub scope_type: String,
    /// The platform, template, streamer or account ID; unused for `global`.
    #[serde(default)]
    pub scope_id: Option<String>,
    /// For a template, the platform to resolve it on.
    #[serde(default)]
    pub platform_id: Option<String>,
}

/// The route a scope's requests take now.
#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveRouteResponse {
    pub kind: RouteKind,
    /// The saved proxy, for a `proxy` route.
    pub proxy: Option<ProxyName>,
    /// Which setting decided the route.
    pub source: RouteSource,
}

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    ProxyRouteState: FromRef<S>,
{
    Router::new()
        .route("/", get(list_proxies).post(create_proxy))
        .route("/system", get(system_proxy))
        .route("/effective", get(effective_route))
        .route("/test", post(test_proxy))
        .route(
            "/{id}",
            get(get_proxy).patch(update_proxy).delete(delete_proxy),
        )
}

#[utoipa::path(
    get,
    path = "/api/proxies",
    tag = "proxies",
    responses((status = 200, description = "Saved proxies", body = Vec<ProxyResponse>)),
    security(("bearer_auth" = []))
)]
pub async fn list_proxies(
    State(state): State<ProxyRouteState>,
) -> ApiResult<Json<Vec<ProxyResponse>>> {
    Ok(Json(
        state
            .proxies
            .list()
            .await?
            .into_iter()
            .map(|(entry, usage)| ProxyResponse::new(entry, usage))
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/proxies/{id}",
    tag = "proxies",
    params(("id" = String, Path, description = "Proxy ID")),
    responses(
        (status = 200, description = "The proxy and the routes naming it", body = ProxyDetailResponse),
        (status = 404, description = "Not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_proxy(
    State(state): State<ProxyRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProxyDetailResponse>> {
    let (entry, references) = state.proxies.get(&id).await?;
    Ok(Json(ProxyDetailResponse {
        proxy: ProxyResponse::new(entry, references.count()),
        references,
    }))
}

#[utoipa::path(
    post,
    path = "/api/proxies",
    tag = "proxies",
    request_body = CreateProxyRequest,
    responses(
        (status = 201, description = "Proxy saved", body = ProxyResponse),
        (status = 409, description = "PROXY_NAME_TAKEN or PROXY_DUPLICATE", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "PROXY_INVALID", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_proxy(
    State(state): State<ProxyRouteState>,
    Json(request): Json<CreateProxyRequest>,
) -> ApiResult<(StatusCode, Json<ProxyResponse>)> {
    let endpoint = ProxyEndpoint {
        url: request.url,
        username: request.username.filter(|username| !username.is_empty()),
        password: request.password,
    };
    let entry = state.proxies.create(&request.name, &endpoint).await?;
    Ok((StatusCode::CREATED, Json(ProxyResponse::new(entry, 0))))
}

#[utoipa::path(
    patch,
    path = "/api/proxies/{id}",
    tag = "proxies",
    params(("id" = String, Path, description = "Proxy ID")),
    request_body = UpdateProxyRequest,
    responses(
        (status = 200, description = "Proxy updated", body = ProxyResponse),
        (status = 409, description = "PROXY_STALE_VERSION, PROXY_NAME_TAKEN or PROXY_DUPLICATE", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "PROXY_INVALID", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_proxy(
    State(state): State<ProxyRouteState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateProxyRequest>,
) -> ApiResult<Json<ProxyResponse>> {
    let entry = state
        .proxies
        .update(
            &id,
            request.expected_version,
            ProxyUpdate {
                name: request.name,
                url: request.url,
                username: request.username,
                password: request.password,
            },
        )
        .await?;
    let (_, references) = state.proxies.get(&entry.id).await?;
    Ok(Json(ProxyResponse::new(entry, references.count())))
}

#[utoipa::path(
    delete,
    path = "/api/proxies/{id}",
    tag = "proxies",
    params(("id" = String, Path, description = "Proxy ID"), DeleteProxyQuery),
    responses(
        (status = 204, description = "Proxy deleted"),
        (status = 409, description = "PROXY_REFERENCED (details.references lists the routes naming it) or PROXY_STALE_VERSION", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_proxy(
    State(state): State<ProxyRouteState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteProxyQuery>,
) -> ApiResult<StatusCode> {
    state.proxies.delete(&id, query.expected_version).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/proxies/system",
    tag = "proxies",
    description = "The proxy the server's environment configures, read at startup. The system route uses it for danmu and throttling; in-process clients also follow the operating system's proxy settings where the platform provides them.",
    responses((status = 200, description = "Detected environment proxy", body = SystemProxySummary)),
    security(("bearer_auth" = []))
)]
pub async fn system_proxy() -> Json<SystemProxySummary> {
    Json(SystemProxy::current().summary())
}

#[utoipa::path(
    get,
    path = "/api/proxies/effective",
    tag = "proxies",
    params(EffectiveRouteQuery),
    responses(
        (status = 200, description = "The route the scope's requests take", body = EffectiveRouteResponse),
        (status = 404, description = "Scope not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn effective_route(
    State(state): State<ProxyRouteState>,
    Query(query): Query<EffectiveRouteQuery>,
) -> ApiResult<Json<EffectiveRouteResponse>> {
    let id = || {
        query
            .scope_id
            .clone()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| ApiError::validation("scope_id is required"))
    };
    let scope = match query.scope_type.as_str() {
        "global" => RouteScope::Global,
        "platform" => RouteScope::Platform { platform_id: id()? },
        "template" => RouteScope::Template {
            template_id: id()?,
            platform_id: query.platform_id.clone(),
        },
        "streamer" => RouteScope::Streamer { streamer_id: id()? },
        "account" => RouteScope::Account { profile_id: id()? },
        _ => return Err(ApiError::validation("unknown scope_type")),
    };
    let route = state.proxies.effective(&scope).await?;
    Ok(Json(EffectiveRouteResponse {
        kind: route.kind(),
        proxy: route.proxy,
        source: route.source,
    }))
}

#[utoipa::path(
    post,
    path = "/api/proxies/test",
    tag = "proxies",
    description = "Requests a platform's home page or a URL once through a saved or unsaved proxy, with a 10 second limit, without following redirects. Any HTTP status except 407 counts as reachable.",
    request_body = TestProxyRequest,
    responses(
        (status = 200, description = "Check result", body = ProbeOutcome),
        (status = 400, description = "Target not allowed", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "PROXY_INVALID or an unknown platform", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn test_proxy(
    State(state): State<ProxyRouteState>,
    Json(request): Json<TestProxyRequest>,
) -> ApiResult<Json<ProbeOutcome>> {
    let target = match (&request.platform, &request.target_url) {
        (Some(platform), None) => crate::proxies::platform_homepage(platform)
            .and_then(|url| url::Url::parse(url).ok())
            .ok_or_else(|| ApiError::validation("unknown platform"))?,
        (None, Some(target)) => {
            let target = url::Url::parse(target.trim())
                .map_err(|_| ApiError::bad_request("Invalid target URL"))?;
            let allow_private = state
                .config_service
                .get_cached_global_config()
                .await?
                .stream_proxy_allow_private_targets;
            super::stream_proxy::validate_target_url(&target, allow_private).await?;
            target
        }
        _ => {
            return Err(ApiError::validation(
                "give exactly one of platform and target_url",
            ));
        }
    };
    let endpoint = match &request.proxy_id {
        Some(id) => {
            let saved = state.proxies.endpoint(id).await?;
            let username = match &request.username {
                Some(username) => username.clone().filter(|username| !username.is_empty()),
                None => saved.username.clone(),
            };
            let password = match (&username, &request.password) {
                (None, _) => None,
                (Some(_), Some(password)) => Some(password.clone()),
                (Some(_), None) => saved.password.clone(),
            };
            ProxyEndpoint {
                url: request.url.clone().unwrap_or(saved.url),
                username,
                password,
            }
        }
        None => ProxyEndpoint {
            url: request
                .url
                .clone()
                .ok_or_else(|| ApiError::validation("url or proxy_id is required"))?,
            username: request.username.clone().flatten().filter(|u| !u.is_empty()),
            password: request.password.clone(),
        },
    };
    let endpoint = crate::proxies::canonical(&endpoint)?;
    Ok(Json(crate::proxies::probe(&endpoint, &target).await))
}
