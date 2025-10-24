use axum::{routing::get, Router};

use crate::api::AppState;

pub fn global_config_routes() -> Router<AppState> {
    Router::new().route("/", get(get_global_config))
}

async fn get_global_config() -> &'static str {
    "Global configuration"
}