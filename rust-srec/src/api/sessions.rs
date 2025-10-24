use axum::{routing::get, Router};

use crate::api::AppState;

pub fn sessions_routes() -> Router<AppState> {
    Router::new().route("/", get(list_sessions))
}

async fn list_sessions() -> &'static str {
    "List of sessions"
}