//! URL parsing routes for extracting media info.

use axum::{
    Json, Router,
    extract::{FromRef, State},
    routing::post,
};
use platforms_parser::extractor::factory::ExtractorFactory;
use std::time::Duration;
use tracing::{debug, warn};

use crate::api::error::ApiResult;
use crate::api::models::{ParseUrlRequest, ParseUrlResponse};
use crate::api::server::AppState;
use crate::credentials::{
    CredentialScope, CredentialSource, extractor_platform_extras, platform_reauth_extra,
};
use crate::domain::ProxyConfig;
use crate::utils::json::{self, JsonContext};

#[derive(Clone)]
pub struct ParseRouteState {
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
    credential_service: std::sync::Arc<
        crate::credentials::CredentialRefreshService<
            crate::database::repositories::config::SqlxConfigRepository,
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
}

#[derive(Default)]
struct ResolvedExtractorConfig {
    cookies: Option<String>,
    platform_extras: Option<serde_json::Value>,
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
    Json(request): Json<ParseUrlRequest>,
) -> ApiResult<Json<ParseUrlResponse>> {
    let extractor_config =
        resolve_extractor_config_for_url(&state, &request.url, request.cookies.clone()).await;
    let proxy_config = resolve_proxy_config_for_url(&state, &request.url).await;
    let extractor_factory = extractor_factory_for_proxy(&proxy_config);
    let response = process_parse_request(
        &extractor_factory,
        request.url,
        extractor_config.cookies,
        extractor_config.platform_extras,
    )
    .await;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/parse/batch",
    tag = "parse",
    request_body = Vec<ParseUrlRequest>,
    responses(
        (status = 200, description = "URLs parsed", body = Vec<ParseUrlResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn parse_url_batch(
    State(state): State<ParseRouteState>,
    Json(requests): Json<Vec<ParseUrlRequest>>,
) -> ApiResult<Json<Vec<ParseUrlResponse>>> {
    let mut responses = Vec::new();
    for request in requests {
        let extractor_config =
            resolve_extractor_config_for_url(&state, &request.url, request.cookies.clone()).await;
        let proxy_config = resolve_proxy_config_for_url(&state, &request.url).await;
        let extractor_factory = extractor_factory_for_proxy(&proxy_config);
        responses.push(
            process_parse_request(
                &extractor_factory,
                request.url,
                extractor_config.cookies,
                extractor_config.platform_extras,
            )
            .await,
        );
    }
    Ok(Json(responses))
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
) -> ResolvedExtractorConfig {
    let has_explicit_cookies = explicit_cookies.is_some();
    let mut resolved = ResolvedExtractorConfig {
        cookies: explicit_cookies,
        platform_extras: None,
    };

    let config_service = &state.config_service;
    let credential_service = &state.credential_service;

    if let Some(streamer) = state.streamer_manager.get_streamer_by_url(url) {
        match config_service.get_context_for_streamer(&streamer.id).await {
            Ok(context) => {
                let config = &context.config;
                resolved.platform_extras = config.platform_extras.clone();
                if !has_explicit_cookies {
                    resolved.cookies = config.cookies.clone();
                }

                if !has_explicit_cookies && let Some(source) = context.credential_source.as_ref() {
                    match credential_service.check_and_refresh_source(source).await {
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

                match credential_service.check_and_refresh_source(&source).await {
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
    Json(request): Json<crate::api::models::ResolveUrlRequest>,
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

    let proxy_config = resolve_proxy_config_for_url(&state, &request.url).await;
    let extractor_factory = extractor_factory_for_proxy(&proxy_config);
    let extractor_config =
        resolve_extractor_config_for_url(&state, &request.url, request.cookies.clone()).await;

    let extractor = match extractor_factory.create_extractor(
        &request.url,
        extractor_config.cookies,
        extractor_config.platform_extras,
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
        Err(e) => Ok(Json(crate::api::models::ResolveUrlResponse {
            success: false,
            stream_info: None,
            error: Some(format!("Failed to resolve URL: {}", e)),
        })),
    }
}

/// Helper to process a single parse request
async fn process_parse_request(
    extractor_factory: &ExtractorFactory,
    url: String,
    cookies: Option<String>,
    platform_extras: Option<serde_json::Value>,
) -> ParseUrlResponse {
    // Validate URL
    if url.is_empty() {
        return ParseUrlResponse {
            success: false,
            is_live: false,
            media_info: None,
            error: Some("URL cannot be empty".to_string()),
        };
    }

    debug!("Parsing URL: {}", url);

    // Create extractor for the URL
    let extractor = match extractor_factory.create_extractor(&url, cookies.clone(), platform_extras)
    {
        Ok(ext) => ext,
        Err(platforms_parser::extractor::error::ExtractorError::UnsupportedExtractor) => {
            warn!("Unsupported platform for URL: {}", url);
            return ParseUrlResponse {
                success: false,
                is_live: false,
                media_info: None,
                error: Some("Unsupported platform".to_string()),
            };
        }
        Err(e) => {
            warn!("Failed to create extractor for URL {}: {}", url, e);
            return ParseUrlResponse {
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
                        success: false,
                        is_live: false,
                        media_info: None,
                        error: Some(format!("Failed to serialize media info: {}", e)),
                    };
                }
            };

            ParseUrlResponse {
                success: true,
                is_live: media_info.is_live,
                media_info: Some(media_info_value),
                error: None,
            }
        }
        Err(e) => {
            debug!("Failed to extract media info for {}: {}", url, e);

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

async fn resolve_proxy_config_for_url(state: &ParseRouteState, url: &str) -> ProxyConfig {
    let config_service = &state.config_service;

    // Priority 1: streamer merged config (final merged proxy state).
    if let Some(streamer) = state.streamer_manager.get_streamer_by_url(url)
        && let Ok(context) = config_service.get_context_for_streamer(&streamer.id).await
    {
        return context.config.proxy_config.clone();
    }

    // Global proxy config (base for non-streamer requests).
    let global_proxy = config_service
        .get_global_config()
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
