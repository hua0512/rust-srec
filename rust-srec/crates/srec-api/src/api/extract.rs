//! Request extractors whose rejections use the API's JSON error body.
//!
//! Axum's own `Json`, `Query` and `Path` reject malformed input with a
//! plain-text body. These wrappers keep each rejection's status and message
//! and render it as [`ApiError`], so clients can read every failure the same
//! way. Responses still use [`axum::Json`].

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;

use super::error::ApiError;

/// JSON request body; see the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

/// Query string; see the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

/// Path parameters; see the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        Ok(Self(value))
    }
}

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state).await?;
        Ok(Self(value))
    }
}

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    Path<T>: FromRequestParts<S, Rejection = PathRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state).await?;
        Ok(Self(value))
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        let code = match &rejection {
            JsonRejection::JsonSyntaxError(_) => "INVALID_JSON",
            JsonRejection::MissingJsonContentType(_) => "UNSUPPORTED_MEDIA_TYPE",
            _ if status == StatusCode::PAYLOAD_TOO_LARGE => "PAYLOAD_TOO_LARGE",
            _ => "INVALID_REQUEST_BODY",
        };
        ApiError::new(status, code, rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        ApiError::new(rejection.status(), "INVALID_QUERY", rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        let status = rejection.status();
        // A handler whose route does not supply the parameters it extracts is a
        // server bug, not a malformed request.
        let code = if status.is_server_error() {
            "INTERNAL_ERROR"
        } else {
            "INVALID_PATH"
        };
        ApiError::new(status, code, rejection.body_text())
    }
}
