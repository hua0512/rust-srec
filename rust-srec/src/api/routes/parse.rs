//! URL parsing routes for extracting media info.

use axum::{
    Extension, Json, Router,
    extract::{FromRef, State},
    http::{HeaderMap, HeaderValue},
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
use crate::credentials::{
    CredentialScope, CredentialSource, OperationDeadline, extractor_platform_extras,
    platform_reauth_extra,
};
use crate::domain::ProxyConfig;
use crate::utils::json::{self, JsonContext};

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
    credential_service: std::sync::Arc<crate::credentials::CredentialRefreshService>,
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
            credential_service: state.credential_service.clone(),
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
    let principal = playback_principal(&state, identity)?;
    parse_one(&state, request, &principal)
        .await
        .map(|response| (private_playback_headers(), Json(response)))
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
        let extractor_config = resolve_extractor_config_for_url(
            state,
            &request.url,
            request.cookies.clone(),
            deadline,
        )
        .await;
        let proxy_config = resolve_proxy_config_for_url(state, &request.url).await;
        let extractor_factory = extractor_factory_for_proxy(&proxy_config);
        admit_parse(state, &extractor_config, deadline).await?;
        let platform_id = extractor_config.platform_id.clone();
        let response = process_parse_request(
            &extractor_factory,
            request.url,
            extractor_config.cookies,
            extractor_config.platform_extras,
            extractor_config.extractor,
            |error| observe_parse_error(state, platform_id.as_deref(), error),
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

fn playback_principal(
    state: &ParseRouteState,
    identity: Option<Extension<AuthPrincipal>>,
) -> ApiResult<String> {
    match identity {
        Some(Extension(principal)) => Ok(principal.claims.sub),
        None if !state.auth_enabled => Ok("local-anonymous".into()),
        None => Err(ApiError::unauthorized("Authentication required")),
    }
}

fn private_playback_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers
}

struct ManagedConfig {
    platform_name: String,
    source: crate::services::playback_context::PlaybackSource,
    policy: crate::credentials::ResolvedCredentialPolicy,
    extras: Option<serde_json::Value>,
    extractor: ExtractorSelection,
    proxy: ProxyConfig,
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
        // belongs to the legacy path, which keeps its own URL and config handling.
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
        let platform = match matches.as_slice() {
            [platform] => platform,
            [] if requested_profile.is_none() => return Ok(None),
            [] => return Err(ApiError::validation("No matching platform configuration")),
            _ if requested_profile.is_none()
                && matches
                    .iter()
                    .all(|platform| platform.credential_selection.is_none()) =>
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
        let configured = platform
            .credential_selection
            .as_deref()
            .map(|raw| {
                let value = serde_json::from_str(raw)?;
                ResolvedCredentialPolicy::new(
                    platform.id.clone(),
                    owner.clone(),
                    CredentialSelection::from_value(value)?,
                )
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
        proxy: resolve_proxy_config_for_url(state, url).await,
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
            |snapshot| {
                let url = config.source.url.clone();
                let extras = crate::credentials::managed_authentication_extras(
                    &config.platform_name,
                    config.extras.clone(),
                    &snapshot.material,
                );
                let factory = extractor_factory_for_proxy(&config.proxy);
                let selection = config.extractor;
                async move {
                    let extractor = factory
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
    let current = managed_config(
        state,
        &data.source.url,
        data.source.explicit_profile.as_deref(),
    )
    .await
    .map_err(playback_resolution_error)?
    .ok_or(PlaybackError::RenewalRequired)?;
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
                health.validity == "invalid"
                    || health
                        .cooldown_until
                        .is_some_and(|until| until > crate::database::time::now_ms())
            })
        {
            return Err(PlaybackError::RenewalRequired.into());
        }
    }
    Ok(data)
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
    let principal = playback_principal(&state, identity)?;
    let deadline = OperationDeadline::default();
    let response = tokio::time::timeout_at(deadline.instant(), async {
        let data = state.playback.get(&request.playback_handle, &principal)?;
        let config = managed_config(
            &state,
            &data.source.url,
            data.source.explicit_profile.as_deref(),
        )
        .await
        .map_err(playback_resolution_error)?
        .ok_or(crate::services::playback_context::PlaybackError::RenewalRequired)?;
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
    Ok((private_playback_headers(), Json(response)))
}

async fn admit_parse(
    state: &ParseRouteState,
    config: &ResolvedExtractorConfig,
    deadline: OperationDeadline,
) -> ApiResult<()> {
    state
        .admission
        .admit(
            config.platform_id.as_deref().unwrap_or("unconfigured"),
            deadline,
        )
        .await
        .map_err(|_| parse_deadline_error())
}

/// Unconfigured URLs share one admission bucket across unrelated hosts, so
/// only a stored platform's throttle delays its other callers.
fn observe_parse_error(state: &ParseRouteState, platform_id: Option<&str>, error: &ExtractorError) {
    if let Some(platform_id) = platform_id {
        state.admission.observe(platform_id, error);
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
    let principal = playback_principal(&state, identity)?;
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
    Ok((private_playback_headers(), Json(responses)))
}

/// Resolve authentication and platform-specific extractor configuration for a URL.
///
/// Explicit request cookies take precedence. Streamer configuration is used
/// when the URL is already registered; otherwise the matching platform
/// configuration supplies cookies, extractor extras, and re-login material.
async fn resolve_extractor_config_for_url(
    state: &ParseRouteState,
    url: &str,
    explicit_cookies: Option<String>,
    deadline: OperationDeadline,
) -> ResolvedExtractorConfig {
    let has_explicit_cookies = explicit_cookies.is_some();
    let mut resolved = ResolvedExtractorConfig {
        platform_id: None,
        cookies: explicit_cookies,
        platform_extras: None,
        extractor: ExtractorSelection::default(),
    };

    let config_service = &state.config_service;
    let credential_service = &state.credential_service;

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
                let config = &context.config;
                let platform_name = if has_explicit_cookies {
                    state
                        .config_service
                        .get_platform_config(&streamer.platform_config_id)
                        .await
                        .ok()
                        .map(|platform| platform.platform_name)
                } else {
                    None
                };
                resolved.platform_extras = config.platform_extras.clone().map(|extras| {
                    if has_explicit_cookies {
                        crate::credentials::isolate_platform_authentication_extras(
                            platform_name.as_deref().unwrap_or_default(),
                            extras,
                        )
                    } else {
                        extras
                    }
                });
                resolved.extractor = config.extractor;
                if !has_explicit_cookies {
                    resolved.cookies = config.cookies.clone();
                }

                if !has_explicit_cookies && let Some(source) = context.credential_source.as_ref() {
                    if let Some(owner) = state.streamer_manager.committed_state() {
                        credential_service.bind_committed_streamers(owner);
                    }
                    match credential_service
                        .check_and_refresh_source_until(source, deadline)
                        .await
                    {
                        Ok(Some(new_cookies)) => {
                            resolved.cookies = Some(new_cookies);
                            match &source.scope {
                                CredentialScope::Streamer { .. } => {
                                    config_service.invalidate_streamer(&streamer.id);
                                }
                                CredentialScope::Template { template_id, .. } => {
                                    if let Err(error) =
                                        config_service.invalidate_template(template_id).await
                                    {
                                        warn!(
                                            %error,
                                            "Failed to invalidate template config after credential refresh"
                                        );
                                    }
                                }
                                CredentialScope::Platform { platform_id, .. } => {
                                    if let Err(error) =
                                        config_service.invalidate_platform(platform_id).await
                                    {
                                        warn!(
                                            %error,
                                            "Failed to invalidate platform config after credential refresh"
                                        );
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(
                                %error,
                                streamer_id = %streamer.id,
                                "Failed to refresh streamer credentials while parsing URL"
                            );
                        }
                    }
                }

                if resolved.cookies.is_some() {
                    debug!(
                        "Using cookies from streamer config for URL: {} (streamer: {})",
                        url, streamer.name
                    );
                }
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
        if !has_explicit_cookies {
            resolved.cookies = platform_config
                .cookies
                .clone()
                .filter(|value| !value.trim().is_empty());
        }

        let platform_specific = platform_config
            .platform_specific_config
            .as_deref()
            .and_then(|config| serde_json::from_str::<serde_json::Value>(config).ok());
        resolved.platform_extras = platform_specific.clone().map(extractor_platform_extras);
        if has_explicit_cookies {
            resolved.platform_extras = resolved.platform_extras.map(|extras| {
                crate::credentials::isolate_platform_authentication_extras(
                    &platform_config.platform_name,
                    extras,
                )
            });
        }

        if !has_explicit_cookies {
            let refresh_token = platform_specific
                .as_ref()
                .and_then(|config| config.get("refresh_token"))
                .and_then(|token| token.as_str())
                .map(String::from);
            let access_token = platform_specific
                .as_ref()
                .and_then(|config| config.get("access_token"))
                .and_then(|token| token.as_str())
                .map(String::from);
            let reauth_extra =
                platform_reauth_extra(&platform_config.platform_name, platform_specific.as_ref());

            if resolved.cookies.is_some() || reauth_extra.is_some() {
                let source = CredentialSource::new(
                    CredentialScope::Platform {
                        platform_id: platform_config.id.clone(),
                        platform_name: platform_config.platform_name.clone(),
                    },
                    resolved.cookies.clone().unwrap_or_default(),
                    refresh_token,
                    platform_config.platform_name.clone(),
                )
                .with_access_token(access_token)
                .with_reauth_extra(reauth_extra);

                match credential_service
                    .check_and_refresh_source_until(&source, deadline)
                    .await
                {
                    Ok(Some(new_cookies)) => {
                        resolved.cookies = Some(new_cookies);
                        if let Err(error) = config_service
                            .invalidate_platform(&platform_config.id)
                            .await
                        {
                            warn!(
                                %error,
                                platform_id = %platform_config.id,
                                "Failed to invalidate platform config after credential refresh"
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        warn!(
                            %error,
                            platform = %platform_config.platform_name,
                            "Failed to refresh platform credentials while parsing URL"
                        );
                    }
                }
            }
        }

        if resolved.cookies.is_some() {
            debug!(
                "Using cookies from platform config for URL: {} (platform: {})",
                url, platform_name
            );
        }
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
    identity: Option<Extension<AuthPrincipal>>,
    Json(request): Json<crate::api::models::ResolveUrlRequest>,
) -> ApiResult<(HeaderMap, Json<crate::api::models::ResolveUrlResponse>)> {
    let principal = playback_principal(&state, identity)?;
    let deadline = OperationDeadline::default();
    let response = tokio::time::timeout_at(deadline.instant(), async {
        match request {
            crate::api::models::ResolveUrlRequest::Managed(request) => {
                validate_playback(&state, &request.playback_handle, &principal).await?;
                let playback = state.playback.resolve(
                    &request.playback_handle,
                    &principal,
                    &request.stream_id,
                )?;
                Ok(Json(crate::api::models::ResolveUrlResponse {
                    success: true,
                    stream_info: None,
                    error: None,
                    playback: Some(playback),
                }))
            }
            crate::api::models::ResolveUrlRequest::Legacy(request) => {
                if request.cookies.is_none()
                    && managed_config(&state, &request.url, None).await?.is_some()
                {
                    return Err(ApiError::conflict(
                        "Managed playback requires a playback handle and server-issued stream ID",
                    ));
                }
                resolve_one(&state, request, deadline).await
            }
        }
    })
    .await
    .map_err(|_| parse_deadline_error())??;
    Ok((private_playback_headers(), response))
}

async fn resolve_one(
    state: &ParseRouteState,
    request: crate::api::models::LegacyResolveUrlRequest,
    deadline: OperationDeadline,
) -> ApiResult<Json<crate::api::models::ResolveUrlResponse>> {
    if request.url.is_empty() {
        return Ok(Json(crate::api::models::ResolveUrlResponse {
            playback: None,
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
                    playback: None,
                    success: false,
                    stream_info: None,
                    error: Some(format!("Invalid stream_info: {}", e)),
                }));
            }
        };

    let proxy_config = resolve_proxy_config_for_url(state, &request.url).await;
    let extractor_factory = extractor_factory_for_proxy(&proxy_config);
    let extractor_config =
        resolve_extractor_config_for_url(state, &request.url, request.cookies.clone(), deadline)
            .await;
    admit_parse(state, &extractor_config, deadline).await?;

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
                playback: None,
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
                playback: None,
                success: true,
                stream_info: Some(val),
                error: None,
            })),
            Err(e) => Ok(Json(crate::api::models::ResolveUrlResponse {
                playback: None,
                success: false,
                stream_info: None,
                error: Some(format!("Failed to serialize updated stream info: {}", e)),
            })),
        },
        Err(e) => {
            observe_parse_error(state, platform_id.as_deref(), &e);
            Ok(Json(crate::api::models::ResolveUrlResponse {
                playback: None,
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

fn extractor_factory_for_proxy(proxy_config: &ProxyConfig) -> ExtractorFactory {
    let client = crate::utils::http_client::build_platforms_client(proxy_config, Duration::ZERO, 0);
    ExtractorFactory::new(client)
}

pub(super) async fn resolve_proxy_config_for_url(
    state: &ParseRouteState,
    url: &str,
) -> ProxyConfig {
    let config_service = &state.config_service;

    // Priority 1: streamer merged config (final merged proxy state).
    if let Some(streamer) = state
        .streamer_manager
        .get_streamer_by_url(url)
        .filter(|streamer| !streamer.is_deleted())
        && let Ok(context) = config_service.get_context_for_streamer(&streamer.id).await
    {
        return context.config.proxy_config.clone();
    }

    // Global proxy config (base for non-streamer requests).
    let global_proxy = config_service
        .get_cached_global_config()
        .await
        .map(|global_config| {
            json::parse_or_default(
                &global_config.proxy_config,
                JsonContext::StreamerConfig {
                    streamer_id: "<parse>",
                    scope: "global",
                    scope_id: None,
                    field: "proxy_config",
                },
                "Invalid JSON config; using defaults",
            )
        })
        .unwrap_or_default();

    // Platform override (global -> platform) when URL is recognized.
    use crate::domain::value_objects::StreamerUrl;
    if let Ok(streamer_url) = StreamerUrl::new(url)
        && let Some(platform_name) = streamer_url.platform()
        && let Ok(platform_configs) = config_service.list_platform_configs().await
        && let Some(platform_config) = platform_configs
            .into_iter()
            .find(|c| c.platform_name.eq_ignore_ascii_case(platform_name))
    {
        let platform_proxy: Option<ProxyConfig> = json::parse_optional(
            platform_config.proxy_config.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: "<parse>",
                scope: "platform",
                scope_id: Some(&platform_config.id),
                field: "proxy_config",
            },
            "Invalid JSON config; ignoring",
        );

        if let Some(proxy) = platform_proxy {
            return proxy;
        }
    }

    global_proxy
}

#[cfg(test)]
mod tests {
    mod legacy_cookies;
    use super::*;
    use crate::config::{ConfigEventBroadcaster, ConfigService};
    use crate::credentials::CredentialRefreshService;
    use crate::credentials::test_support::StubCredentialManager;
    use crate::database::models::StreamerDbModel;
    use crate::database::repositories::{
        SqlxConfigRepository, SqlxCredentialStore, SqlxStreamerRepository, StreamerRepository as _,
    };
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::streamer::{StreamerManager, manager::StreamerUpdateParams};
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
    fn managed_resolve_cannot_be_mixed_with_client_owned_media_or_material() {
        for extra in ["url", "stream_info", "cookies", "credential_id", "headers"] {
            let mut request = serde_json::json!({"playback_handle":"opaque", "stream_id":"issued"});
            request[extra] = serde_json::json!("client-owned");
            assert!(
                serde_json::from_value::<crate::api::models::ResolveUrlRequest>(request).is_err()
            );
        }
        assert!(
            serde_json::from_value::<crate::api::models::ResolveUrlRequest>(
                serde_json::json!({"playback_handle":"opaque", "stream_id":"issued"})
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn playback_revalidates_principal_profile_revision_and_current_policy() {
        use crate::credentials::{
            CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
            CredentialSelection, CredentialSnapshot,
        };
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let execution = test_execution(&pool);
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        let profile = execution
            .repository()
            .create(
                &owner,
                "platform-bilibili",
                "Account",
                true,
                &CredentialMaterial {
                    cookies: "session=private".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
            )
            .await
            .unwrap();
        let selection = CredentialSelection::Fixed {
            credential_id: profile.id.clone(),
        };
        sqlx::query(
            "UPDATE platform_config SET credential_selection=? WHERE id='platform-bilibili'",
        )
        .bind(serde_json::to_string(&selection).unwrap())
        .execute(&pool)
        .await
        .unwrap();
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
            .update(&profile.id, profile.version, Some("Renamed"), None, None)
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
            .update(&profile.id, renamed.version, None, None, Some(&replacement))
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
        sqlx::query("UPDATE platform_config SET credential_selection='{\"mode\":\"none\"}' WHERE id='platform-bilibili'").execute(&pool).await.unwrap();
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
            Arc::new(CredentialRefreshService::new(Arc::new(
                SqlxCredentialStore::new(pool.clone(), pool.clone()),
            ))),
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
            credential_service: Arc::new(CredentialRefreshService::new(Arc::new(
                SqlxCredentialStore::new(pool.clone(), pool.clone()),
            ))),
            streamer_manager: Arc::new(StreamerManager::new(
                streamers,
                ConfigEventBroadcaster::new(),
            )),
        }
    }
    const STREAMER_URL: &str = "https://live.bilibili.com/1";

    #[tokio::test]
    async fn unmanaged_sources_fall_through_to_the_legacy_parse_path() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let state = route_state(&pool, test_execution(&pool));
        let unregistered = "https://live.bilibili.com/2";
        // The legacy path ignores malformed extras and invalid URLs, and picks the
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
        sqlx::query("UPDATE platform_config SET credential_selection = '{\"mode\":\"none\"}' WHERE id = 'platform-bilibili-upper'")
            .execute(&pool).await.unwrap();
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
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let state = ParseRouteState {
            auth_enabled: false,
            playback: Arc::new(
                crate::services::playback_context::PlaybackContextService::default(),
            ),
            execution: test_execution(&pool),
            admission: Arc::new(crate::credentials::PlatformAdmission::from_config(
                &crate::monitor::StreamMonitorConfig::default(),
            )),
            config_service: Arc::new(ConfigService::new(
                config_repo.clone(),
                streamer_repo.clone(),
            )),
            credential_service: Arc::new(CredentialRefreshService::new(Arc::new(
                SqlxCredentialStore::new(pool.clone(), pool.clone()),
            ))),
            streamer_manager: Arc::new(StreamerManager::new(
                streamer_repo,
                ConfigEventBroadcaster::new(),
            )),
        };
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

    /// Resolving a registered streamer's URL refreshes its credentials through
    /// `CredentialStore::update_credentials`, which rewrites `streamer_specific_config` with its
    /// own SQL and leaves the manager's metadata cache on the previous document. The reload keeps
    /// the refreshed credentials from being rebuilt away by the next streamer edit.
    #[tokio::test]
    async fn refreshed_credentials_survive_a_later_streamer_edit() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let mut model = StreamerDbModel::new("Streamer", STREAMER_URL, "platform-bilibili");
        model.id = STREAMER_ID.to_string();
        model.streamer_specific_config =
            Some(r#"{"cookies":"SESSDATA=old","refresh_token":"refresh-old"}"#.to_string());

        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        streamer_repo.create_streamer(&model).await.unwrap();

        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            ConfigEventBroadcaster::new(),
        ));
        streamer_manager.hydrate().await.unwrap();

        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let mut credential_service = CredentialRefreshService::new(Arc::new(
            SqlxCredentialStore::new(pool.clone(), pool.clone()),
        ));
        credential_service.register_manager(Arc::new(StubCredentialManager::new(
            "bilibili",
            "SESSDATA=new",
            "refresh-new",
        )));

        let state = ParseRouteState {
            auth_enabled: false,
            playback: Arc::new(
                crate::services::playback_context::PlaybackContextService::default(),
            ),
            execution: test_execution(&pool),
            admission: credential_service.admission(),
            config_service: Arc::new(ConfigService::new(config_repo, streamer_repo.clone())),
            credential_service: Arc::new(credential_service),
            streamer_manager: streamer_manager.clone(),
        };

        let resolved = resolve_extractor_config_for_url(
            &state,
            STREAMER_URL,
            None,
            OperationDeadline::default(),
        )
        .await;
        assert_eq!(resolved.cookies.as_deref(), Some("SESSDATA=new"));

        // A later edit rebuilds the whole streamers row from the manager's metadata cache.
        streamer_manager
            .partial_update_streamer(StreamerUpdateParams {
                id: STREAMER_ID.to_string(),
                name: Some("Renamed".to_string()),
                url: None,
                platform_config_id: None,
                template_config_id: None,
                priority: None,
                state: None,
                streamer_specific_config: None,
            })
            .await
            .expect("rename succeeds");

        let row = streamer_repo
            .get_streamer(STREAMER_ID)
            .await
            .expect("row exists");
        let config: serde_json::Value = serde_json::from_str(
            row.streamer_specific_config
                .as_deref()
                .expect("streamer carries a config document"),
        )
        .expect("config document is valid JSON");
        assert_eq!(config["cookies"], "SESSDATA=new");
        assert_eq!(config["refresh_token"], "refresh-new");
    }

    #[tokio::test]
    async fn stream_proxy_configuration_uses_source_identity_and_current_overrides() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            ConfigEventBroadcaster::new(),
        ));
        let state = ParseRouteState {
            auth_enabled: false,
            playback: Arc::new(
                crate::services::playback_context::PlaybackContextService::default(),
            ),
            execution: test_execution(&pool),
            admission: Arc::new(crate::credentials::PlatformAdmission::from_config(
                &crate::monitor::StreamMonitorConfig::default(),
            )),
            config_service: Arc::new(ConfigService::new(config_repo, streamer_repo.clone())),
            credential_service: Arc::new(CredentialRefreshService::new(Arc::new(
                SqlxCredentialStore::new(pool.clone(), pool.clone()),
            ))),
            streamer_manager: manager.clone(),
        };
        let global_proxy = ProxyConfig::with_url("http://global-proxy.example:8080");
        let mut global = state.config_service.get_global_config().await.unwrap();
        global.proxy_config = serde_json::to_string(&global_proxy).unwrap();
        state
            .config_service
            .update_global_config(&global)
            .await
            .unwrap();
        assert_eq!(
            resolve_proxy_config_for_url(&state, "https://cdn.example/video").await,
            global_proxy
        );
        let platform_proxy = ProxyConfig::with_url("http://platform-proxy.example:8080");
        let mut platform = state
            .config_service
            .get_platform_config("platform-bilibili")
            .await
            .unwrap();
        platform.proxy_config = Some(serde_json::to_string(&platform_proxy).unwrap());
        state
            .config_service
            .update_platform_config(&platform)
            .await
            .unwrap();
        assert_eq!(
            resolve_proxy_config_for_url(&state, STREAMER_URL).await,
            platform_proxy
        );
        let mut streamer = StreamerDbModel::new("Source", STREAMER_URL, "platform-bilibili");
        streamer.id = STREAMER_ID.to_string();
        streamer.streamer_specific_config =
            Some(serde_json::json!({ "proxy_config": ProxyConfig::disabled() }).to_string());
        streamer_repo.create_streamer(&streamer).await.unwrap();
        manager.hydrate().await.unwrap();
        assert_eq!(
            resolve_proxy_config_for_url(&state, STREAMER_URL).await,
            ProxyConfig::disabled()
        );
        global.proxy_config = serde_json::to_string(&ProxyConfig::disabled()).unwrap();
        state
            .config_service
            .update_global_config(&global)
            .await
            .unwrap();
        assert_eq!(
            resolve_proxy_config_for_url(&state, "https://cdn.example/video").await,
            ProxyConfig::disabled()
        );
    }
}
