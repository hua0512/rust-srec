//! URL parsing routes for extracting media info.

use axum::{Json, Router, routing::post};
use platforms_parser::extractor::factory::ExtractorFactory;
use tracing::{debug, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{ParseUrlRequest, ParseUrlResponse};
use crate::api::server::AppState;

/// Create the parse router.
pub fn router() -> Router<AppState> {
    Router::new().route("/", post(parse_url))
}

/// Parse a URL and extract media info.
///
/// POST /api/parse
///
/// Uses the platforms_parser crate to extract media information from the given URL.
/// Returns the full MediaInfo structure as JSON.
async fn parse_url(Json(request): Json<ParseUrlRequest>) -> ApiResult<Json<ParseUrlResponse>> {
    // Validate URL
    if request.url.is_empty() {
        return Err(ApiError::validation("URL cannot be empty"));
    }

    debug!("Parsing URL: {}", request.url);

    // Create HTTP client and extractor factory
    let client = platforms_parser::extractor::create_client_builder(None)
        .build()
        .unwrap();
    let extractor_factory = ExtractorFactory::new(client);

    // Create extractor for the URL
    let extractor = match extractor_factory.create_extractor(&request.url, request.cookies, None) {
        Ok(ext) => ext,
        Err(platforms_parser::extractor::error::ExtractorError::UnsupportedExtractor) => {
            warn!("Unsupported platform for URL: {}", request.url);
            return Ok(Json(ParseUrlResponse {
                success: false,
                is_live: false,
                media_info: None,
                error: Some("Unsupported platform".to_string()),
            }));
        }
        Err(e) => {
            warn!("Failed to create extractor for URL {}: {}", request.url, e);
            return Ok(Json(ParseUrlResponse {
                success: false,
                is_live: false,
                media_info: None,
                error: Some(format!("Failed to create extractor: {}", e)),
            }));
        }
    };

    // Extract media info
    match extractor.extract().await {
        Ok(media_info) => {
            debug!(
                "Successfully extracted media info for {}: is_live={}, streams={}",
                request.url,
                media_info.is_live,
                media_info.streams.len()
            );

            // Convert MediaInfo to serde_json::Value for serialization
            let media_info_value = match media_info.to_value() {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to serialize media info: {}", e);
                    return Ok(Json(ParseUrlResponse {
                        success: false,
                        is_live: false,
                        media_info: None,
                        error: Some(format!("Failed to serialize media info: {}", e)),
                    }));
                }
            };

            Ok(Json(ParseUrlResponse {
                success: true,
                is_live: media_info.is_live,
                media_info: Some(media_info_value),
                error: None,
            }))
        }
        Err(e) => {
            debug!("Failed to extract media info for {}: {}", request.url, e);

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

            Ok(Json(ParseUrlResponse {
                success: false,
                is_live: false,
                media_info: None,
                error: Some(error_message),
            }))
        }
    }
}
