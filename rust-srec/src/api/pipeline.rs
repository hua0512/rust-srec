use axum::{routing::get, Router};

use crate::api::AppState;

pub fn pipeline_routes() -> Router<AppState> {
    Router::new().route("/", get(list_pipeline_outputs))
}

async fn list_pipeline_outputs() -> &'static str {
    "List of pipeline outputs"
}