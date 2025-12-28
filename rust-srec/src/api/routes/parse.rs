//! URL parsing routes for extracting media info.

use axum::{Json, Router, extract::State, routing::post};
use platforms_parser::extractor::factory::ExtractorFactory;
use tracing::{debug, warn};

use crate::api::error::ApiResult;
use crate::api::models::{ParseUrlRequest, ParseUrlResponse};
use crate::api::server::AppState;
use crate::credentials::{CredentialScope, CredentialSource};

/// Create the parse router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(parse_url))
        .route("/batch", post(parse_url_batch))
        .route("/resolve", post(resolve_url))
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
    State(state): State<AppState>,
    Json(request): Json<ParseUrlRequest>,
) -> ApiResult<Json<ParseUrlResponse>> {
    let cookies = resolve_cookies_for_url(&state, &request.url, request.cookies.clone()).await;
    let extractor_factory = extractor_factory(&state);
    let response = process_parse_request(&extractor_factory, request.url, cookies).await;
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
    State(state): State<AppState>,
    Json(requests): Json<Vec<ParseUrlRequest>>,
) -> ApiResult<Json<Vec<ParseUrlResponse>>> {
    let mut responses = Vec::new();
    let extractor_factory = extractor_factory(&state);
    for request in requests {
        let cookies = resolve_cookies_for_url(&state, &request.url, request.cookies.clone()).await;
        responses.push(process_parse_request(&extractor_factory, request.url, cookies).await);
    }
    Ok(Json(responses))
}

/// Resolve cookies for a URL.
///
/// Priority order:
/// 1. Explicitly provided cookies in the request
/// 2. Streamer config cookies (if a matching streamer exists for this URL)
/// 3. Platform config cookies (detected from the URL)
async fn resolve_cookies_for_url(
    state: &AppState,
    url: &str,
    explicit_cookies: Option<String>,
) -> Option<String> {
    // If cookies are explicitly provided, use them
    if explicit_cookies.is_some() {
        return explicit_cookies;
    }

    let config_service = state.config_service.as_ref()?;
    let credential_service = state.credential_service.as_ref();

    // Try to find a matching streamer by URL
    if let Some(streamer_manager) = state.streamer_manager.as_ref()
        && let Some(streamer) = streamer_manager.get_streamer_by_url(url)
    {
        match config_service.get_context_for_streamer(&streamer.id).await {
            Ok(context) => {
                let config = &context.config;
                let mut cookies = config.cookies.clone();

                if let Some(credential_service) = credential_service
                    && let Some(source) = context.credential_source.as_ref()
                    && let Ok(Some(new_cookies)) =
                        credential_service.check_and_refresh_source(source).await
                {
                    cookies = Some(new_cookies);
                    match &source.scope {
                        CredentialScope::Streamer { .. } => {
                            config_service.invalidate_streamer(&streamer.id);
                        }
                        CredentialScope::Template { template_id, .. } => {
                            let _ = config_service.invalidate_template(template_id).await;
                        }
                        CredentialScope::Platform { platform_id, .. } => {
                            let _ = config_service.invalidate_platform(platform_id).await;
                        }
                    }
                }

                if cookies.is_some() {
                    debug!(
                        "Using cookies from streamer config for URL: {} (streamer: {})",
                        url, streamer.name
                    );
                    return cookies;
                }
            }
            Err(e) => {
                warn!("Failed to get config for streamer {}: {}", streamer.id, e);
            }
        }
    }

    // Fallback: Try to detect platform from URL and use platform config cookies
    use crate::domain::value_objects::StreamerUrl;

    if let Ok(streamer_url) = StreamerUrl::new(url)
        && let Some(platform_name) = streamer_url.platform()
    {
        // Find a matching platform config
        if let Ok(platform_configs) = config_service.list_platform_configs().await
            && let Some(platform_config) = platform_configs
                .into_iter()
                .find(|c| c.platform_name.eq_ignore_ascii_case(platform_name))
            && platform_config.cookies.is_some()
        {
            let mut cookies = platform_config.cookies.clone();

            if let Some(credential_service) = credential_service {
                let refresh_token = platform_config
                    .platform_specific_config
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("refresh_token").and_then(|t| t.as_str()).map(String::from));

                if let Some(ref existing) = cookies {
                    let source = CredentialSource::new(
                        CredentialScope::Platform {
                            platform_id: platform_config.id.clone(),
                            platform_name: platform_config.platform_name.clone(),
                        },
                        existing.clone(),
                        refresh_token,
                        platform_config.platform_name.clone(),
                    );

                    if let Ok(Some(new_cookies)) =
                        credential_service.check_and_refresh_source(&source).await
                    {
                        cookies = Some(new_cookies);
                        let _ = config_service.invalidate_platform(&platform_config.id).await;
                    }
                }
            }

            debug!(
                "Using cookies from platform config for URL: {} (platform: {})",
                url, platform_name
            );
            return cookies;
        }
    }

    None
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
    State(state): State<AppState>,
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

    // Create extractor
    let extractor_factory = extractor_factory(&state);

    let extractor = match extractor_factory.create_extractor(&request.url, request.cookies, None) {
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
    let extractor = match extractor_factory.create_extractor(&url, cookies.clone(), None) {
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

fn extractor_factory(state: &AppState) -> ExtractorFactory {
    let client = state
        .http_client
        .clone()
        .unwrap_or_else(AppState::build_http_client);
    ExtractorFactory::new(client)
}
