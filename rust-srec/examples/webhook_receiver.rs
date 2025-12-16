//! Minimal webhook receiver example.
//!
//! Run:
//!   cargo run -p rust-srec --example webhook_receiver
//!
//! Send a test webhook:
//!   $env:WEBHOOK_SECRET="dev-secret"
//!   curl -X POST http://127.0.0.1:3000/webhook -H "content-type: application/json" -H "x-webhook-secret: dev-secret" -d "{\"event\":\"ping\"}"

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

#[derive(Debug, Clone)]
struct WebhookState {
    shared_secret: Option<String>,
    body_limit_bytes: usize,
}

impl WebhookState {
    fn from_env() -> Self {
        let shared_secret = std::env::var("WEBHOOK_SECRET")
            .ok()
            .and_then(|s| (!s.trim().is_empty()).then_some(s));

        let body_limit_bytes = std::env::var("WEBHOOK_BODY_LIMIT_BYTES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1024 * 1024);

        Self {
            shared_secret,
            body_limit_bytes,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let addr: SocketAddr = std::env::var("WEBHOOK_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:12333".to_string())
        .parse()?;

    let state = Arc::new(WebhookState::from_env());

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/webhook", post(webhook))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    info!(%addr, "webhook receiver listening");
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    (
        StatusCode::OK,
        "POST /webhook\n\
         GET  /healthz\n\
         \n\
         Optional env:\n\
         - WEBHOOK_ADDR=127.0.0.1:3000\n\
         - WEBHOOK_SECRET=dev-secret\n\
         - WEBHOOK_BODY_LIMIT_BYTES=1048576\n",
    )
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn webhook(State(state): State<Arc<WebhookState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();

    if let Some(expected) = state.shared_secret.as_deref() {
        let provided = parts
            .headers
            .get("x-webhook-secret")
            .and_then(|v| v.to_str().ok());
        if provided != Some(expected) {
            warn!("rejected webhook: missing/invalid x-webhook-secret");
            return (StatusCode::UNAUTHORIZED, "invalid webhook secret").into_response();
        }
    }

    let bytes = match to_bytes(body, state.body_limit_bytes).await {
        Ok(b) => b,
        Err(err) => {
            warn!(%err, "failed to read request body");
            return (StatusCode::BAD_REQUEST, "invalid body").into_response();
        }
    };

    let parsed_json: Option<Value> = match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(_) => None,
    };

    info!(
        content_type = ?parts.headers.get("content-type").and_then(|v| v.to_str().ok()),
        bytes = bytes.len(),
        json = parsed_json.is_some(),
        "received webhook"
    );

    let response = json!({
        "ok": true,
        "received_bytes": bytes.len(),
        "json": parsed_json,
    });

    info!("json: {:#?}", parsed_json);

    (StatusCode::OK, axum::Json(response)).into_response()
}
