//! Media routes.

use std::fmt::Write as _;
use std::path::PathBuf;

use axum::Router;
use axum::extract::{FromRef, Path, Query, Request, State};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_DISPOSITION;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeFile;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

#[derive(Clone)]
pub struct MediaRouteState {
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    session_repository: std::sync::Arc<dyn crate::database::repositories::SessionRepository>,
}

impl FromRef<AppState> for MediaRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
            session_repository: state.session_repository.clone(),
        }
    }
}

/// Create the media router.
pub fn router() -> Router<AppState> {
    Router::new().route("/{id}/content", get(get_media_content))
}

#[derive(serde::Deserialize)]
pub struct MediaContentQuery {
    pub token: Option<String>,
    /// `1` or `true` asks for the file to be saved rather than displayed.
    pub download: Option<String>,
}

impl MediaContentQuery {
    fn wants_attachment(&self) -> bool {
        self.download.as_deref().is_some_and(|value| {
            value.eq_ignore_ascii_case("1") || value.eq_ignore_ascii_case("true")
        })
    }
}

/// Build a `Content-Disposition: attachment` value naming the media output's own file.
///
/// Both forms are emitted: the quoted ASCII `filename` for clients that read only that
/// one, and the RFC 5987 `filename*` carrying the original UTF-8 name. A stored path may
/// use either separator regardless of the host, so the last component is taken from both;
/// anything that could terminate the quoted form or smuggle a header (quotes, backslashes,
/// control characters) is dropped, and a name left empty falls back to the output id.
fn attachment_disposition(file_path: &str, id: &str) -> String {
    let base = file_path.rsplit(['/', '\\']).next().unwrap_or_default();
    let sanitized: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\' | '/'))
        .collect();
    let name = if sanitized.trim().is_empty() {
        id
    } else {
        sanitized.trim()
    };

    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        percent_encode_attr_char(name)
    )
}

/// Percent-encode `value` down to RFC 5987 `attr-char`, the only bytes an `ext-value` may
/// carry unescaped.
fn percent_encode_attr_char(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            )
        {
            encoded.push(*byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Turn a stored `media_outputs.file_path` into a path usable by the std/tokio APIs.
///
/// Windows note: some parts of the pipeline/tooling may emit extended-length paths
/// like `\\?\C:\...`. While this is valid for Win32 APIs, it can be a portability
/// footgun across libraries and runtimes. Normalize it to a regular path when possible.
///
/// Shared by [`get_media_content`] and the media output deletion in
/// [`crate::api::routes::pipeline::jobs`] so both resolve a stored path identically.
pub(crate) fn normalize_media_path(file_path: &str) -> PathBuf {
    let path = PathBuf::from(file_path);
    if cfg!(windows)
        && let Some(s) = path.to_str()
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            // `\\?\UNC\server\share\...` -> `\\server\share\...`
            return PathBuf::from(format!(r"\\{}", rest));
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            // `\\?\C:\...` -> `C:\...`
            return PathBuf::from(rest);
        }
    }
    path
}

#[utoipa::path(
    get,
    path = "/api/media/{id}/content",
    tag = "media",
    params(
        ("id" = String, Path, description = "Media output ID"),
        ("download" = Option<String>, Query, description = "Set to `1` or `true` to receive the file as an attachment")
    ),
    responses(
        (status = 200, description = "Media file content"),
        (status = 404, description = "Media not found", body = crate::api::error::ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_media_content(
    State(state): State<MediaRouteState>,
    Path(id): Path<String>,
    Query(query): Query<MediaContentQuery>,
    req: Request,
) -> ApiResult<Response> {
    let headers = req.headers();
    crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers,
        query.token.as_deref(),
        crate::api::auth_request::AccessPolicy::Read,
    )
    .await?;

    let session_repo = &state.session_repository;

    // Query media output to get file path
    let media = session_repo
        .get_media_output(&id)
        .await
        .map_err(ApiError::from)?;

    let path = normalize_media_path(&media.file_path);

    let exists = tokio::fs::try_exists(&path)
        .await
        .map_err(|error| ApiError::from(crate::Error::io_path("try_exists", &path, error)))?;
    if !exists {
        return Err(ApiError::not_found(format!("Media file not found: {}", id)));
    }

    let wants_attachment = query.wants_attachment();

    match ServeFile::new(path).try_call(req).await {
        Ok(response) => {
            let mut response = response.into_response();
            if wants_attachment
                && let Ok(value) =
                    HeaderValue::from_str(&attachment_disposition(&media.file_path, &id))
            {
                response.headers_mut().insert(CONTENT_DISPOSITION, value);
            }
            Ok(response)
        }
        Err(error) => Err(ApiError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{
        models::{
            ApiKeyAccessLevel, LiveSessionDbModel, MediaFileType, MediaOutputDbModel,
            StreamerDbModel,
        },
        repositories::{
            SessionRepository, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository,
        },
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn media_router_authenticates_read_keys_and_never_falls_back_from_a_header() {
        let fixture = crate::api::auth_request::tests::fixture().await;
        let (_, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "media-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let state = MediaRouteState {
            auth_service: Some(fixture.service.clone()),
            session_repository: std::sync::Arc::new(SqlxSessionRepository::new(
                fixture.pool.clone(),
                fixture.pool.clone(),
            )),
        };
        let app = Router::new()
            .route("/{id}/content", get(get_media_content))
            .with_state(state);
        // An absent output distinguishes authorization success from rejection without filesystem IO.
        for (query, header, expected) in [
            (raw.as_str(), None, StatusCode::NOT_FOUND),
            (
                "invalid",
                Some(format!("Bearer {raw}")),
                StatusCode::NOT_FOUND,
            ),
            (
                raw.as_str(),
                Some("Basic malformed".to_owned()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                raw.as_str(),
                Some("Bearer invalid".to_owned()),
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let mut request = Request::builder().uri(format!("/absent/content?token={query}"));
            if let Some(header) = header {
                request = request.header("Authorization", header);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        fixture.pool.close().await;
    }

    #[tokio::test]
    async fn media_content_is_an_attachment_only_when_the_download_flag_is_set() {
        let fixture = crate::api::auth_request::tests::fixture().await;
        let (_, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "media-download",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let sessions = std::sync::Arc::new(SqlxSessionRepository::new(
            fixture.pool.clone(),
            fixture.pool.clone(),
        ));
        let streamer = StreamerDbModel::new(
            "Streamer",
            "https://example.com/streamer",
            "platform-twitch",
        );
        SqlxStreamerRepository::new(fixture.pool.clone(), fixture.pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
        let session = LiveSessionDbModel::new(&streamer.id);
        sessions.create_session(&session).await.unwrap();

        let directory = tempfile::tempdir().unwrap();
        // Non-ASCII exercises the `filename*` form; the name stays legal on every
        // platform the suite runs on.
        let file_path = directory.path().join("récording one.mp4");
        tokio::fs::write(&file_path, b"payload").await.unwrap();
        let output = MediaOutputDbModel::new(
            &session.id,
            file_path.to_str().unwrap(),
            MediaFileType::Video,
            7,
        );
        sessions.create_media_output(&output).await.unwrap();

        let state = MediaRouteState {
            auth_service: Some(fixture.service.clone()),
            session_repository: sessions.clone(),
        };
        let app = Router::new()
            .route("/{id}/content", get(get_media_content))
            .with_state(state);

        for (query, expected) in [
            ("", None),
            ("&download=0", None),
            (
                "&download=1",
                Some(
                    "attachment; filename=\"r_cording one.mp4\"; \
                     filename*=UTF-8''r%C3%A9cording%20one.mp4",
                ),
            ),
            (
                "&download=true",
                Some(
                    "attachment; filename=\"r_cording one.mp4\"; \
                     filename*=UTF-8''r%C3%A9cording%20one.mp4",
                ),
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/{}/content?token={raw}{query}", output.id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(CONTENT_DISPOSITION)
                    .map(|value| value.to_str().unwrap()),
                expected,
                "query {query:?}"
            );
        }
        fixture.pool.close().await;
    }

    #[test]
    fn attachment_disposition_falls_back_to_the_output_id() {
        assert_eq!(
            attachment_disposition("/output/", "abc"),
            "attachment; filename=\"abc\"; filename*=UTF-8''abc"
        );
        assert_eq!(
            attachment_disposition(r"C:\output\clip.mp4", "abc"),
            "attachment; filename=\"clip.mp4\"; filename*=UTF-8''clip.mp4"
        );
    }

    #[test]
    fn attachment_disposition_strips_characters_that_would_break_the_header() {
        assert_eq!(
            attachment_disposition("/output/r\"ec\tord\ning.mp4", "abc"),
            "attachment; filename=\"recording.mp4\"; filename*=UTF-8''recording.mp4"
        );
    }
}
