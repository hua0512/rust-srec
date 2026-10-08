//! URL parsing routes for extracting media info.

use axum::{
    Extension, Json, Router,
    extract::{FromRef, State},
    http::HeaderMap,
    routing::post,
};
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::{ExtractorFactory, ExtractorSelection};
use std::time::Duration;
use tracing::{debug, warn};

use crate::api::auth_service::AuthPrincipal;
use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{ParseUrlRequest, ParseUrlResponse};
use crate::api::server::AppState;
use crate::credentials::OperationDeadline;
use crate::proxies::ProxyTarget;

#[derive(Clone)]
pub struct ParseRouteState {
    auth_enabled: bool,
    playback: std::sync::Arc<crate::services::playback_context::PlaybackContextService>,
    execution: std::sync::Arc<crate::credentials::CredentialExecutionService>,
    admission: std::sync::Arc<crate::credentials::PlatformAdmission>,
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

impl FromRef<AppState> for ParseRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_enabled: state.auth_service.is_some(),
            playback: state.playback_contexts.clone(),
            execution: state.credential_execution.clone(),
            admission: state.platform_admission.clone(),
            config_service: state.config_service.clone(),
            streamer_manager: state.streamer_manager.clone(),
        }
    }
}

/// Create the parse router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(parse_url))
        .route("/batch", post(parse_url_batch))
        .route("/resolve", post(resolve_url))
        .route("/playback/renew", post(renew_playback))
}

#[derive(Default)]
struct ResolvedExtractorConfig {
    platform_id: Option<String>,
    cookies: Option<String>,
    platform_extras: Option<serde_json::Value>,
    /// Mirrors the recording path so a parse preview uses the same extractor a session would.
    extractor: ExtractorSelection,
}

const MAX_PARSE_BATCH_SIZE: usize = 100;

fn validate_parse_batch_size(count: usize) -> ApiResult<()> {
    if count > MAX_PARSE_BATCH_SIZE {
        return Err(ApiError::validation(format!(
            "A batch may contain at most {MAX_PARSE_BATCH_SIZE} URLs"
        )));
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/parse",
    tag = "parse",
    request_body = ParseUrlRequest,
    responses(
        (status = 200, description = "URL parsed", body = ParseUrlResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn parse_url(
    State(state): State<ParseRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Json(request): Json<ParseUrlRequest>,
) -> ApiResult<(HeaderMap, Json<ParseUrlResponse>)> {
    let principal = super::request_principal(state.auth_enabled, identity)?;
    parse_one(&state, request, &principal)
        .await
        .map(|response| (super::private_response_headers(), Json(response)))
}

async fn parse_one(
    state: &ParseRouteState,
    request: ParseUrlRequest,
    principal: &str,
) -> ApiResult<ParseUrlResponse> {
    if request.cookies.is_some() && request.credential_id.is_some() {
        return Err(ApiError::validation(
            "credential_id and cookies are mutually exclusive",
        ));
    }
    let deadline = OperationDeadline::default();
    tokio::time::timeout_at(deadline.instant(), async {
        if request.cookies.is_none()
            && let Some(config) =
                managed_config(state, &request.url, request.credential_id.as_deref()).await?
        {
            return extract_managed(state, principal, config, None, deadline).await;
        }
        let extractor_config =
            resolve_extractor_config_for_url(state, &request.url, request.cookies.clone()).await;
        let route = resolve_route_for_url(state, &request.url).await?;
        let extractor_factory = extractor_factory_for_proxy(&route.target)?;
        admit_parse(state, &extractor_config, &route, deadline).await?;
        let platform_id = extractor_config.platform_id.clone();
        let response = process_parse_request(
            &extractor_factory,
            request.url,
            extractor_config.cookies,
            extractor_config.platform_extras,
            extractor_config.extractor,
            |error| observe_parse_error(state, platform_id.as_deref(), &route, error),
        )
        .await;
        Ok(response)
    })
    .await
    .map_err(|_| parse_deadline_error())?
}

fn parse_deadline_error() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "CREDENTIAL_DEADLINE_EXCEEDED",
        "Platform admission or extraction deadline exceeded; retry later",
    )
}

struct ManagedConfig {
    platform_name: String,
    source: crate::services::playback_context::PlaybackSource,
    policy: crate::credentials::ResolvedCredentialPolicy,
    extras: Option<serde_json::Value>,
    extractor: ExtractorSelection,
    /// The source's route; an account with its own route replaces it.
    route: crate::proxies::ResolvedRoute,
}

async fn managed_config(
    state: &ParseRouteState,
    url: &str,
    requested_profile: Option<&str>,
) -> ApiResult<Option<ManagedConfig>> {
    use crate::credentials::{CredentialOwner, CredentialSelection, ResolvedCredentialPolicy};
    let (owner, platform_id, platform_name, configured, extras, extractor) = if let Some(streamer) =
        state
            .streamer_manager
            .get_streamer_by_url(url)
            .filter(|row| !row.is_deleted())
    {
        let context = state
            .config_service
            .get_context_for_streamer(&streamer.id)
            .await
            .map_err(ApiError::from)?;
        let platform = state
            .config_service
            .get_platform_config(&streamer.platform_config_id)
            .await
            .map_err(ApiError::from)?;
        (
            CredentialOwner::Streamer {
                streamer_id: streamer.id.clone(),
            },
            streamer.platform_config_id.clone(),
            platform.platform_name,
            context.config.credential_policy.clone(),
            context.config.platform_extras.clone(),
            context.config.extractor,
        )
    } else {
        // Without an explicit profile, anything short of a managed platform policy
        // belongs to the unmanaged path, which keeps its own URL and config handling.
        let page = match crate::domain::StreamerUrl::new(url) {
            Ok(page) => page,
            Err(_) if requested_profile.is_none() => return Ok(None),
            Err(_) => return Err(ApiError::validation("Invalid source URL")),
        };
        let Some(name) = page.platform() else {
            if requested_profile.is_some() {
                return Err(ApiError::validation(
                    "A profile requires an identified platform",
                ));
            }
            return Ok(None);
        };
        let matches = state
            .config_service
            .list_platform_configs()
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .filter(|row| row.platform_name.eq_ignore_ascii_case(name))
            .collect::<Vec<_>>();
        let selections: std::collections::HashMap<String, CredentialSelection> = state
            .config_service
            .list_credential_selections()
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .filter(|stored| matches!(stored.owner, CredentialOwner::Platform { .. }))
            .map(|stored| (stored.platform_id, stored.selection))
            .collect();
        let platform = match matches.as_slice() {
            [platform] => platform,
            [] if requested_profile.is_none() => return Ok(None),
            [] => return Err(ApiError::validation("No matching platform configuration")),
            _ if requested_profile.is_none()
                && matches
                    .iter()
                    .all(|platform| !selections.contains_key(&platform.id)) =>
            {
                return Ok(None);
            }
            _ => {
                return Err(ApiError::conflict(
                    "Platform name is ambiguous; register this URL with an explicit platform",
                ));
            }
        };
        let owner = CredentialOwner::Platform {
            platform_id: platform.id.clone(),
        };
        let configured = selections
            .get(&platform.id)
            .map(|selection| {
                ResolvedCredentialPolicy::new(platform.id.clone(), owner.clone(), selection.clone())
            })
            .transpose()
            .map_err(ApiError::from)?;
        if requested_profile.is_none() && configured.is_none() {
            return Ok(None);
        }
        let extras = platform
            .platform_specific_config
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(ApiError::from)?;
        let extractor = platform
            .extractor
            .as_deref()
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        (
            owner,
            platform.id.clone(),
            platform.platform_name.clone(),
            configured,
            extras,
            extractor,
        )
    };
    let configured_generation = configured.as_ref().map(|policy| policy.generation.clone());
    let policy = if let Some(id) = requested_profile {
        if id.trim().is_empty() {
            return Err(ApiError::validation("credential_id must be nonblank"));
        }
        if !state
            .execution
            .repository()
            .accessible(&owner, &platform_id)
            .await
            .map_err(ApiError::from)?
            .iter()
            .any(|profile| profile.id == id)
        {
            return Err(ApiError::validation(
                "Credential profile is not accessible for this source",
            ));
        }
        ResolvedCredentialPolicy::new(
            platform_id,
            owner.clone(),
            CredentialSelection::Fixed {
                credential_id: id.to_owned(),
            },
        )
        .map_err(ApiError::from)?
    } else if let Some(policy) = configured {
        policy
    } else {
        return Ok(None);
    };
    Ok(Some(ManagedConfig {
        platform_name,
        source: crate::services::playback_context::PlaybackSource {
            url: url.to_owned(),
            owner,
            explicit_profile: requested_profile.map(str::to_owned),
            configured_generation,
        },
        policy,
        extras,
        extractor,
        route: resolve_route_for_url(state, url).await?,
    }))
}

async fn extract_managed(
    state: &ParseRouteState,
    principal: &str,
    config: ManagedConfig,
    binding: Option<&crate::credentials::CredentialBinding>,
    deadline: OperationDeadline,
) -> ApiResult<ParseUrlResponse> {
    let extraction = state
        .execution
        .execute(
            &config.policy,
            binding,
            binding.is_none(),
            deadline,
            &config.route,
            |snapshot| {
                let url = config.source.url.clone();
                let extras = crate::credentials::managed_authentication_extras(
                    &config.platform_name,
                    config.extras.clone(),
                    &snapshot.material,
                );
                let factory = extractor_factory_for_proxy(&snapshot.route.target);
                let selection = config.extractor;
                async move {
                    let extractor = factory?
                        .create_extractor(
                            &url,
                            Some(snapshot.material.cookies.clone()),
                            extras,
                            selection,
                        )
                        .map_err(crate::Error::from)?;
                    let mut media = extractor.extract().await.map_err(crate::Error::from)?;
                    if media.streams.len() > crate::services::playback_context::MAX_PLAYBACK_STREAMS
                    {
                        return Err(crate::Error::validation(
                            "Too many playback stream candidates",
                        ));
                    }
                    let mut resolved = Vec::new();
                    let mut last_error = None;
                    for mut stream in std::mem::take(&mut media.streams) {
                        match extractor.get_url(&mut stream).await {
                            Ok(()) => resolved.push(stream),
                            Err(
                                error @ (ExtractorError::Authentication { .. }
                                | ExtractorError::RateLimited { .. }),
                            ) => return Err(error.into()),
                            Err(ExtractorError::NoStreamsFound) => {
                                media.is_live = false;
                                resolved.clear();
                                break;
                            }
                            Err(error) => last_error = Some(error),
                        }
                    }
                    if media.is_live
                        && resolved.is_empty()
                        && let Some(error) = last_error
                    {
                        return Err(error.into());
                    }
                    media.streams = resolved;
                    // Only the provider's explicit login result replaces the
                    // account bundle. CDN cookies belong to their media variant.
                    let session_cookies = media
                        .extras
                        .as_ref()
                        .and_then(|extras| extras.get("session_cookies"))
                        .filter(|cookies| !cookies.is_empty())
                        .cloned();
                    Ok(crate::credentials::Extracted {
                        preserve_health: !media.is_live,
                        value: media,
                        session_cookies,
                    })
                }
            },
        )
        .await
        .map_err(ApiError::from)?;
    let is_live = extraction.value.is_live;
    let playback = state.playback.insert(
        principal,
        config.source,
        extraction.snapshot,
        extraction.value,
    )?;
    Ok(ParseUrlResponse {
        playback: Some(playback),
        success: true,
        is_live,
        media_info: None,
        error: None,
    })
}

pub(crate) async fn validate_playback(
    state: &ParseRouteState,
    handle: &str,
    principal: &str,
) -> ApiResult<std::sync::Arc<crate::services::playback_context::PlaybackData>> {
    use crate::services::playback_context::PlaybackError;
    let data = state.playback.get(handle, principal)?;
    let current = current_managed_config(state, &data.source).await?;
    if current.policy.generation != data.snapshot.binding.policy.generation
        || current.source.configured_generation != data.source.configured_generation
        || current.source.owner != data.source.owner
    {
        return Err(PlaybackError::RenewalRequired.into());
    }
    state
        .execution
        .repository()
        .validate_selection(&current.policy)
        .await
        .map_err(playback_profile_error)?;
    if let crate::credentials::CredentialIdentity::Profile { profile_id } =
        &data.snapshot.binding.identity
    {
        let profile = state
            .execution
            .repository()
            .get(profile_id)
            .await
            .map_err(playback_profile_error)?;
        if !profile.enabled || profile.revision as u64 != data.snapshot.binding.revision {
            return Err(PlaybackError::RenewalRequired.into());
        }
        if state
            .execution
            .repository()
            .health(profile_id)
            .await
            .map_err(ApiError::from)?
            .is_some_and(|health| {
                health.validity == crate::credentials::CredentialValidity::Invalid
            })
        {
            return Err(PlaybackError::RenewalRequired.into());
        }
    }
    Ok(data)
}

/// The managed configuration a playback context's source resolves to now. A
/// source that is no longer managed needs a fresh parse.
async fn current_managed_config(
    state: &ParseRouteState,
    source: &crate::services::playback_context::PlaybackSource,
) -> ApiResult<ManagedConfig> {
    Ok(
        managed_config(state, &source.url, source.explicit_profile.as_deref())
            .await
            .map_err(playback_resolution_error)?
            .ok_or(crate::services::playback_context::PlaybackError::RenewalRequired)?,
    )
}

fn playback_resolution_error(error: ApiError) -> ApiError {
    if matches!(
        error.status,
        axum::http::StatusCode::NOT_FOUND
            | axum::http::StatusCode::CONFLICT
            | axum::http::StatusCode::UNPROCESSABLE_ENTITY
    ) {
        crate::services::playback_context::PlaybackError::RenewalRequired.into()
    } else {
        error
    }
}

fn playback_profile_error(error: crate::Error) -> ApiError {
    match error {
        crate::Error::NotFound { .. }
        | crate::Error::CredentialProfile(
            crate::credentials::ProfileError::InvalidOwner
            | crate::credentials::ProfileError::SourceChanged
            | crate::credentials::ProfileError::StaleVersion,
        ) => crate::services::playback_context::PlaybackError::RenewalRequired.into(),
        error => ApiError::from(error),
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenewPlaybackRequest {
    pub playback_handle: String,
}

#[utoipa::path(post, path = "/api/parse/playback/renew", tag = "parse", request_body = RenewPlaybackRequest, responses((status = 200, body = ParseUrlResponse)), security(("bearer_auth" = [])))]
pub async fn renew_playback(
    State(state): State<ParseRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Json(request): Json<RenewPlaybackRequest>,
) -> ApiResult<(HeaderMap, Json<ParseUrlResponse>)> {
    let principal = super::request_principal(state.auth_enabled, identity)?;
    let deadline = OperationDeadline::default();
    let response = tokio::time::timeout_at(deadline.instant(), async {
        let data = state.playback.get(&request.playback_handle, &principal)?;
        let config = current_managed_config(&state, &data.source).await?;
        extract_managed(
            &state,
            &principal,
            config,
            Some(&data.snapshot.binding),
            deadline,
        )
        .await
    })
    .await
    .map_err(|_| parse_deadline_error())??;
    Ok((super::private_response_headers(), Json(response)))
}

async fn admit_parse(
    state: &ParseRouteState,
    config: &ResolvedExtractorConfig,
    route: &crate::proxies::ResolvedRoute,
    deadline: OperationDeadline,
) -> ApiResult<()> {
    state
        .admission
        .admit(
            config.platform_id.as_deref().unwrap_or("unconfigured"),
            &route.key,
            deadline,
        )
        .await
        .map_err(|_| parse_deadline_error())
}

/// Unconfigured URLs share one admission bucket across unrelated hosts, so
/// only a stored platform's throttle delays its other callers on the route.
fn observe_parse_error(
    state: &ParseRouteState,
    platform_id: Option<&str>,
    route: &crate::proxies::ResolvedRoute,
    error: &ExtractorError,
) {
    if let Some(platform_id) = platform_id {
        state.admission.observe(platform_id, route, error);
    }
}

#[utoipa::path(
    post,
    path = "/api/parse/batch",
    tag = "parse",
    request_body = Vec<ParseUrlRequest>,
    responses(
        (status = 200, description = "URLs parsed", body = Vec<ParseUrlResponse>),
        (status = 422, description = "Batch exceeds 100 URLs", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn parse_url_batch(
    State(state): State<ParseRouteState>,
    identity: Option<Extension<AuthPrincipal>>,
    Json(requests): Json<Vec<ParseUrlRequest>>,
) -> ApiResult<(HeaderMap, Json<Vec<ParseUrlResponse>>)> {
    validate_parse_batch_size(requests.len())?;
    let principal = super::request_principal(state.auth_enabled, identity)?;
    let mut responses = Vec::new();
    for request in requests {
        responses.push(
            parse_one(&state, request, &principal)
                .await
                .unwrap_or_else(|error| ParseUrlResponse {
                    playback: None,
                    success: false,
                    is_live: false,
                    media_info: None,
                    error: Some(error.message),
                }),
        );
    }
    Ok((super::private_response_headers(), Json(responses)))
}

/// Resolve authentication and platform-specific extractor configuration for a URL.
///
/// Unmanaged requests: explicit request cookies, if any, are the only
/// authentication. Streamer configuration supplies extractor settings when the
/// URL is already registered; otherwise the matching platform configuration.
async fn resolve_extractor_config_for_url(
    state: &ParseRouteState,
    url: &str,
    explicit_cookies: Option<String>,
) -> ResolvedExtractorConfig {
    let mut resolved = ResolvedExtractorConfig {
        platform_id: None,
        cookies: explicit_cookies,
        platform_extras: None,
        extractor: ExtractorSelection::default(),
    };
    let config_service = &state.config_service;

    // A streamer marked deleted still owns its url until the reaper removes the
    // row, but its configuration is no longer the user's; resolve as if the url
    // belonged to no streamer.
    if let Some(streamer) = state
        .streamer_manager
        .get_streamer_by_url(url)
        .filter(|streamer| !streamer.is_deleted())
    {
        resolved.platform_id = Some(streamer.platform_config_id.clone());
        match config_service.get_context_for_streamer(&streamer.id).await {
            Ok(context) => {
                // The resolver already removed account fields from these extras.
                resolved.platform_extras = context.config.platform_extras.clone();
                resolved.extractor = context.config.extractor;
                return resolved;
            }
            Err(error) => {
                warn!(
                    %error,
                    streamer_id = %streamer.id,
                    "Failed to get streamer config while parsing URL"
                );
            }
        }
    }

    use crate::domain::value_objects::StreamerUrl;

    if let Ok(streamer_url) = StreamerUrl::new(url)
        && let Some(platform_name) = streamer_url.platform()
        && let Ok(platform_configs) = config_service.list_platform_configs().await
        && let Some(platform_config) = platform_configs
            .into_iter()
            .find(|config| config.platform_name.eq_ignore_ascii_case(platform_name))
    {
        resolved.platform_id = Some(platform_config.id.clone());
        resolved.platform_extras = platform_config
            .platform_specific_config
            .as_deref()
            .and_then(|config| serde_json::from_str::<serde_json::Value>(config).ok())
            .map(|extras| {
                crate::credentials::isolate_platform_authentication_extras(
                    &platform_config.platform_name,
                    extras,
                )
            });
    }

    resolved
}

#[utoipa::path(
    post,
    path = "/api/parse/resolve",
    tag = "parse",
    request_body = crate::api::models::ResolveUrlRequest,
    responses(
        (status = 200, description = "URL resolved", body = crate::api::models::ResolveUrlResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn resolve_url(
    State(state): State<ParseRouteState>,
    Json(request): Json<crate::api::models::ResolveUrlRequest>,
) -> ApiResult<(HeaderMap, Json<crate::api::models::ResolveUrlResponse>)> {
    let deadline = OperationDeadline::default();
    let response = tokio::time::timeout_at(deadline.instant(), async {
        // Managed sources resolve every stream during parse; resolving one here
        // would extract without the account's credentials.
        if request.cookies.is_none() && managed_config(&state, &request.url, None).await?.is_some()
        {
            return Err(ApiError::conflict(
                "Managed playback streams are resolved during parse; parse the source again",
            ));
        }
        resolve_one(&state, request, deadline).await
    })
    .await
    .map_err(|_| parse_deadline_error())??;
    Ok((super::private_response_headers(), response))
}

async fn resolve_one(
    state: &ParseRouteState,
    request: crate::api::models::ResolveUrlRequest,
    deadline: OperationDeadline,
) -> ApiResult<Json<crate::api::models::ResolveUrlResponse>> {
    if request.url.is_empty() {
        return Ok(Json(crate::api::models::ResolveUrlResponse {
            success: false,
            stream_info: None,
            error: Some("URL cannot be empty".to_string()),
        }));
    }

    // Deserialize stream_info from Value to StreamInfo
    let mut stream_info: platforms_parser::media::StreamInfo =
        match serde_json::from_value(request.stream_info) {
            Ok(info) => info,
            Err(e) => {
                return Ok(Json(crate::api::models::ResolveUrlResponse {
                    success: false,
                    stream_info: None,
                    error: Some(format!("Invalid stream_info: {}", e)),
                }));
            }
        };

    let route = resolve_route_for_url(state, &request.url).await?;
    let extractor_factory = extractor_factory_for_proxy(&route.target)?;
    let extractor_config =
        resolve_extractor_config_for_url(state, &request.url, request.cookies.clone()).await;
    admit_parse(state, &extractor_config, &route, deadline).await?;

    let platform_id = extractor_config.platform_id.clone();
    let extractor = match extractor_factory.create_extractor(
        &request.url,
        extractor_config.cookies,
        extractor_config.platform_extras,
        extractor_config.extractor,
    ) {
        Ok(ext) => ext,
        Err(e) => {
            return Ok(Json(crate::api::models::ResolveUrlResponse {
                success: false,
                stream_info: None,
                error: Some(format!("Failed to create extractor: {}", e)),
            }));
        }
    };

    // Call get_url
    match extractor.get_url(&mut stream_info).await {
        Ok(_) => match serde_json::to_value(&stream_info) {
            Ok(val) => Ok(Json(crate::api::models::ResolveUrlResponse {
                success: true,
                stream_info: Some(val),
                error: None,
            })),
            Err(e) => Ok(Json(crate::api::models::ResolveUrlResponse {
                success: false,
                stream_info: None,
                error: Some(format!("Failed to serialize updated stream info: {}", e)),
            })),
        },
        Err(e) => {
            observe_parse_error(state, platform_id.as_deref(), &route, &e);
            Ok(Json(crate::api::models::ResolveUrlResponse {
                success: false,
                stream_info: None,
                error: Some(format!("Failed to resolve URL: {}", e)),
            }))
        }
    }
}

/// Helper to process a single parse request
async fn process_parse_request(
    extractor_factory: &ExtractorFactory,
    url: String,
    cookies: Option<String>,
    platform_extras: Option<serde_json::Value>,
    extractor_selection: ExtractorSelection,
    on_extract_error: impl FnOnce(&ExtractorError),
) -> ParseUrlResponse {
    // Validate URL
    if url.is_empty() {
        return ParseUrlResponse {
            playback: None,
            success: false,
            is_live: false,
            media_info: None,
            error: Some("URL cannot be empty".to_string()),
        };
    }

    debug!("Parsing URL: {}", url);

    // Create extractor for the URL
    let extractor = match extractor_factory.create_extractor(
        &url,
        cookies.clone(),
        platform_extras,
        extractor_selection,
    ) {
        Ok(ext) => ext,
        Err(platforms_parser::extractor::error::ExtractorError::UnsupportedExtractor) => {
            warn!("Unsupported platform for URL: {}", url);
            return ParseUrlResponse {
                playback: None,
                success: false,
                is_live: false,
                media_info: None,
                error: Some("Unsupported platform".to_string()),
            };
        }
        Err(e) => {
            warn!("Failed to create extractor for URL {}: {}", url, e);
            return ParseUrlResponse {
                playback: None,
                success: false,
                is_live: false,
                media_info: None,
                error: Some(format!("Failed to create extractor: {}", e)),
            };
        }
    };

    // Extract media info
    match extractor.extract().await {
        Ok(media_info) => {
            debug!(
                "Successfully extracted media info for {}: is_live={}, streams={}",
                url,
                media_info.is_live,
                media_info.streams.len()
            );

            // Convert MediaInfo to serde_json::Value for serialization
            let media_info_value = match media_info.to_value() {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to serialize media info: {}", e);
                    return ParseUrlResponse {
                        playback: None,
                        success: false,
                        is_live: false,
                        media_info: None,
                        error: Some(format!("Failed to serialize media info: {}", e)),
                    };
                }
            };

            ParseUrlResponse {
                playback: None,
                success: true,
                is_live: media_info.is_live,
                media_info: Some(media_info_value),
                error: None,
            }
        }
        Err(e) => {
            debug!("Failed to extract media info for {}: {}", url, e);
            on_extract_error(&e);

            // Check for specific error types
            let error_message = match &e {
                platforms_parser::extractor::error::ExtractorError::StreamerNotFound => {
                    "Streamer not found".to_string()
                }
                platforms_parser::extractor::error::ExtractorError::StreamerBanned => {
                    "Streamer is banned".to_string()
                }
                platforms_parser::extractor::error::ExtractorError::AgeRestrictedContent => {
                    "Content is age-restricted".to_string()
                }
                platforms_parser::extractor::error::ExtractorError::RegionLockedContent => {
                    "Content is region-locked".to_string()
                }
                platforms_parser::extractor::error::ExtractorError::PrivateContent => {
                    "Content is private".to_string()
                }
                platforms_parser::extractor::error::ExtractorError::NoStreamsFound => {
                    "Streamer is offline (no streams found)".to_string()
                }
                _ => format!("Extraction failed: {}", e),
            };

            ParseUrlResponse {
                playback: None,
                success: false,
                is_live: false,
                media_info: None,
                error: Some(error_message),
            }
        }
    }
}

fn extractor_factory_for_proxy(
    proxy: &ProxyTarget,
) -> Result<ExtractorFactory, crate::proxies::ProxyError> {
    let client = crate::utils::http_client::build_platforms_client(proxy, Duration::ZERO, 0)?;
    Ok(ExtractorFactory::new(client).with_proxy(proxy.clone()))
}

/// The route requests for `url` take: a registered streamer's, else the
/// route of the platform the URL belongs to, else the global route. A route
/// that cannot be resolved fails the request rather than connecting directly.
pub(super) async fn resolve_route_for_url(
    state: &ParseRouteState,
    url: &str,
) -> ApiResult<crate::proxies::ResolvedRoute> {
    let config_service = &state.config_service;
    if let Some(streamer) = state
        .streamer_manager
        .get_streamer_by_url(url)
        .filter(|streamer| !streamer.is_deleted())
    {
        return Ok(config_service
            .get_context_for_streamer(&streamer.id)
            .await?
            .config
            .proxy_route
            .clone());
    }
    use crate::domain::value_objects::StreamerUrl;
    let platform_id = match StreamerUrl::new(url)
        .ok()
        .and_then(|url| url.platform().map(str::to_owned))
    {
        Some(platform_name) => config_service
            .list_platform_configs()
            .await?
            .into_iter()
            .find(|config| config.platform_name.eq_ignore_ascii_case(&platform_name))
            .map(|config| config.id),
        None => None,
    };
    Ok(config_service
        .resolve_scope_route(platform_id.as_deref())
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigEventBroadcaster, ConfigService};
    use crate::credentials::CredentialProviderRegistry;
    use crate::database::models::StreamerDbModel;
    use crate::database::repositories::credential_selections::test_support::set_platform_selection;
    use crate::database::repositories::{
        SqlxConfigRepository, SqlxStreamerRepository, StreamerRepository as _,
    };
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::streamer::StreamerManager;
    use std::sync::Arc;

    const STREAMER_ID: &str = "streamer-under-test";

    #[test]
    fn playback_invalidation_requests_renewal_but_preserves_infrastructure_errors() {
        use axum::http::StatusCode;
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::CONFLICT,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            let error =
                playback_resolution_error(ApiError::new(status, "INVALID_SCOPE", "scope retired"));
            assert_eq!(error.status, StatusCode::CONFLICT);
            assert_eq!(error.code, "PLAYBACK_RENEWAL_REQUIRED");
        }
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let error = playback_resolution_error(ApiError::new(
                status,
                "INFRASTRUCTURE_FAILURE",
                "retry later",
            ));
            assert_eq!(error.status, status);
            assert_eq!(error.code, "INFRASTRUCTURE_FAILURE");
        }
        let retired = playback_profile_error(crate::Error::CredentialProfile(
            crate::credentials::ProfileError::InvalidOwner,
        ));
        assert_eq!(retired.code, "PLAYBACK_RENEWAL_REQUIRED");
        let database =
            playback_profile_error(crate::Error::Database("database unavailable".into()));
        assert_eq!(database.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn resolve_requests_accept_only_client_owned_media() {
        assert!(
            serde_json::from_value::<crate::api::models::ResolveUrlRequest>(serde_json::json!({
                "url": "https://live.example/room",
                "stream_info": {},
                "playback_handle": "opaque",
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::api::models::ResolveUrlRequest>(serde_json::json!({
                "url": "https://live.example/room",
                "stream_info": {},
                "cookies": "a=b",
            }))
            .is_ok()
        );
    }

    #[tokio::test]
    async fn playback_revalidates_principal_profile_revision_and_current_policy() {
        use crate::credentials::{
            CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialSelection,
            CredentialSnapshot,
        };
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let execution = test_execution(&pool);
        let profile = execution
            .repository()
            .create(
                "platform-bilibili",
                "Account",
                true,
                &CredentialMaterial {
                    cookies: "session=private".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        let selection = CredentialSelection::Fixed {
            credential_id: profile.id.clone(),
        };
        set_platform_selection(&pool, "platform-bilibili", &selection).await;
        let state = route_state(&pool, execution);
        let config = managed_config(&state, STREAMER_URL, None)
            .await
            .unwrap()
            .unwrap();
        let snapshot = CredentialSnapshot {
            binding: CredentialBinding {
                identity: CredentialIdentity::Profile {
                    profile_id: profile.id.clone(),
                },
                revision: profile.revision as u64,
                epoch: 0,
                policy: config.policy,
            },
            material: profile.material().unwrap(),
            route: crate::proxies::ResolvedRoute::default(),
        };
        let (_, _, media) = crate::services::playback_context::test_bundle(
            "https://cdn.test/master?signature=private",
        );
        let context = state
            .playback
            .insert("alice", config.source, snapshot, media)
            .unwrap();
        assert_eq!(
            validate_playback(&state, &context.handle, "bob")
                .await
                .err()
                .unwrap()
                .status,
            axum::http::StatusCode::FORBIDDEN
        );
        assert!(
            validate_playback(&state, &context.handle, "alice")
                .await
                .is_ok()
        );
        let renamed = state
            .execution
            .repository()
            .update(
                &profile.id,
                profile.version,
                Some("Renamed"),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            validate_playback(&state, &context.handle, "alice")
                .await
                .is_ok(),
            "label-only edits keep a bound bundle current"
        );
        let replacement = CredentialMaterial {
            cookies: "session=replacement".into(),
            refresh_token: None,
            access_token: None,
            reauth_config: None,
        };
        state
            .execution
            .repository()
            .update(
                &profile.id,
                renamed.version,
                None,
                None,
                Some(&replacement),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            validate_playback(&state, &context.handle, "alice")
                .await
                .err()
                .unwrap()
                .code,
            "PLAYBACK_RENEWAL_REQUIRED"
        );
        // Policy invalidation is independent of material revision checks.
        set_platform_selection(&pool, "platform-bilibili", &CredentialSelection::None).await;
        assert_eq!(
            validate_playback(&state, &context.handle, "alice")
                .await
                .err()
                .unwrap()
                .code,
            "PLAYBACK_RENEWAL_REQUIRED"
        );
    }
    fn test_execution(
        pool: &sqlx::SqlitePool,
    ) -> Arc<crate::credentials::CredentialExecutionService> {
        Arc::new(crate::credentials::CredentialExecutionService::new(
            Arc::new(
                crate::database::repositories::CredentialProfileRepository::new(
                    pool.clone(),
                    pool.clone(),
                ),
            ),
            Arc::new(CredentialProviderRegistry::new()),
        ))
    }
    fn route_state(
        pool: &sqlx::SqlitePool,
        execution: Arc<crate::credentials::CredentialExecutionService>,
    ) -> ParseRouteState {
        let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        ParseRouteState {
            auth_enabled: false,
            playback: Arc::new(
                crate::services::playback_context::PlaybackContextService::default(),
            ),
            execution,
            admission: Arc::new(crate::credentials::PlatformAdmission::from_config(
                &crate::monitor::StreamMonitorConfig::default(),
            )),
            config_service: Arc::new(ConfigService::new(
                Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
                streamers.clone(),
            )),
            streamer_manager: Arc::new(StreamerManager::new(
                streamers,
                ConfigEventBroadcaster::new(),
            )),
        }
    }
    const STREAMER_URL: &str = "https://live.bilibili.com/1";

    #[tokio::test]
    async fn unmanaged_sources_fall_through_to_the_unmanaged_parse_path() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let state = route_state(&pool, test_execution(&pool));
        let unregistered = "https://live.bilibili.com/2";
        // The unmanaged path ignores malformed extras and invalid URLs, and picks the
        // first case-insensitive platform match. None of these may fail here.
        sqlx::query("UPDATE platform_config SET platform_specific_config = 'not json' WHERE id = 'platform-bilibili'")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES ('platform-bilibili-upper', 'BILIBILI')")
            .execute(&pool).await.unwrap();
        assert!(
            managed_config(&state, unregistered, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            managed_config(&state, "not a url", None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            managed_config(&state, "not a url", Some("profile"))
                .await
                .err()
                .unwrap()
                .status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        // A managed policy on either case variant makes the platform ambiguous.
        set_platform_selection(
            &pool,
            "platform-bilibili-upper",
            &crate::credentials::CredentialSelection::None,
        )
        .await;
        assert_eq!(
            managed_config(&state, unregistered, None)
                .await
                .err()
                .unwrap()
                .status,
            axum::http::StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn oversized_parse_batch_is_rejected_before_async_extraction_or_credentials() {
        assert!(validate_parse_batch_size(0).is_ok());
        assert!(validate_parse_batch_size(MAX_PARSE_BATCH_SIZE).is_ok());
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        let state = route_state(&pool, test_execution(&pool));
        let request = ParseUrlRequest {
            credential_id: None,
            url: STREAMER_URL.to_string(),
            cookies: None,
        };
        // Occupying the only connection would suspend a premature configuration/credential
        // lookup. The invalid batch must finish in its first poll instead.
        let connection = pool.acquire().await.unwrap();
        let mut response = Box::pin(parse_url_batch(
            State(state),
            None,
            Json(vec![request; MAX_PARSE_BATCH_SIZE + 1]),
        ));
        match futures::poll!(response.as_mut()) {
            std::task::Poll::Ready(Err(error)) => {
                assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY)
            }
            _ => panic!("invalid batch must be rejected before processing any URL"),
        }
        drop(response);
        drop(connection);
        pool.close().await;
    }

    #[tokio::test]
    async fn requests_for_a_url_take_the_route_of_its_streamer_platform_or_global() {
        use crate::database::repositories::proxies::{self, RouteOwner};
        use crate::proxies::{ProxyRoute, RouteKind, RouteSource};
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let state = route_state(&pool, test_execution(&pool));
        let manager = state.streamer_manager.clone();
        let global_proxy =
            proxies::save_for_test(&pool, "global", "http://global-proxy.example:8080").await;
        let platform_proxy =
            proxies::save_for_test(&pool, "platform", "http://platform-proxy.example:8080").await;
        let global = state.config_service.get_global_config().await.unwrap();
        state
            .config_service
            .update_global_config_with_route(
                &global,
                Some(&ProxyRoute::Proxy {
                    id: global_proxy.clone(),
                }),
            )
            .await
            .unwrap();
        let unknown = resolve_route_for_url(&state, "https://cdn.example/video")
            .await
            .unwrap();
        assert_eq!(unknown.source, RouteSource::Global);
        assert_eq!(
            unknown
                .target
                .endpoint()
                .map(|endpoint| endpoint.url.as_str()),
            Some("http://global-proxy.example:8080")
        );
        let platform = state
            .config_service
            .get_platform_config("platform-bilibili")
            .await
            .unwrap();
        state
            .config_service
            .update_platform_config_scoped(
                &platform,
                None,
                Some(&ProxyRoute::Proxy {
                    id: platform_proxy.clone(),
                }),
            )
            .await
            .unwrap();
        let recognized = resolve_route_for_url(&state, STREAMER_URL).await.unwrap();
        assert_eq!(recognized.source, RouteSource::Platform);
        assert_eq!(recognized.proxy_name(), Some("platform"));

        let mut streamer = StreamerDbModel::new("Source", STREAMER_URL, "platform-bilibili");
        streamer.id = STREAMER_ID.to_string();
        streamer.streamer_specific_config =
            Some(serde_json::json!({ "proxy_route": {"kind": "direct"} }).to_string());
        streamer_repo.create_streamer(&streamer).await.unwrap();
        manager.hydrate().await.unwrap();
        let registered = resolve_route_for_url(&state, STREAMER_URL).await.unwrap();
        assert_eq!(
            (registered.kind(), registered.source),
            (RouteKind::Direct, RouteSource::Streamer)
        );
        // The route is stored in its own columns, not in the document.
        let stored = streamer_repo.get_streamer(STREAMER_ID).await.unwrap();
        assert!(
            !stored
                .streamer_specific_config
                .unwrap_or_default()
                .contains("proxy_route")
        );
        assert_eq!(
            proxies::route_of(
                &mut pool.acquire().await.unwrap(),
                &RouteOwner::Streamer(STREAMER_ID.into())
            )
            .await
            .unwrap(),
            ProxyRoute::Direct
        );
    }
}
