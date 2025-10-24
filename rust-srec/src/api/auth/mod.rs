use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequestParts, State},
    http::{request::Parts, Method, Request, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use serde_json::{json, Value};
use tracing::error;

use crate::{
    database::repositories::api_key_repository::ApiKeyRepository, domain::api_key::Role,
    AppState,
};

pub async fn auth_middleware<B>(
    State(state): State<Arc<AppState>>,
    req: Request<B>,
    next: Next<B>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|header| header.to_str().ok());

    let api_key = if let Some(auth_header) = auth_header {
        if auth_header.starts_with("Bearer ") {
            auth_header[7..].to_string()
        } else {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid Authorization header format" })),
            ));
        }
    } else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Authorization header missing" })),
        ));
    };

    let api_key_repository = ApiKeyRepository::new(state.db_service.pool.clone());
    let key_hash = ApiKeyRepository::hash_api_key(&api_key);

    match api_key_repository.get_api_key_by_hash(&key_hash).await {
        Ok(Some(api_key_record)) => {
            let role = api_key_record.role;
            let method = req.method();

            if method == Method::GET {
                // Readonly and Admin can access GET endpoints
                Ok(next.run(req).await)
            } else if role == Role::Admin {
                // Only Admin can access non-GET endpoints
                Ok(next.run(req).await)
            } else {
                Err((
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "Insufficient permissions" })),
                ))
            }
        }
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid API key" })),
        )),
        Err(e) => {
            error!("Failed to validate API key: {:?}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to validate API key" })),
            ))
        }
    }
}