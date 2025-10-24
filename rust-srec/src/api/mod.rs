use std::{net::SocketAddr, sync::Arc};

use axum::{middleware, response::Html, routing::get, Router};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{config::ConfigService, database::DatabaseService};

use self::{
    auth::auth_middleware,
    global_config::global_config_routes,
    notification_channels::{
        notification_channels_routes, CreateNotificationChannel, UpdateNotificationChannel,
    },
    pipeline::pipeline_routes,
    sessions::sessions_routes,
    streamers::{streamers_routes, CreateStreamer, ErrorResponse, StreamerStatus},
    templates::{templates_routes, CreateTemplate, UpdateTemplate},
};

pub mod auth;
pub mod error;
pub mod global_config;
pub mod notification_channels;
pub mod pipeline;
pub mod sessions;
pub mod streamers;
pub mod templates;

#[derive(OpenApi)]
#[openapi(
    paths(
        streamers::list_streamers,
        streamers::create_streamer,
        streamers::get_streamer,
        streamers::update_streamer,
        streamers::delete_streamer,
        streamers::get_streamer_statuses,
        streamers::trigger_check,
        streamers::clear_error_state,
        templates::list_templates,
        templates::create_template,
        templates::get_template,
        templates::update_template,
        templates::delete_template,
        notification_channels::list_notification_channels,
        notification_channels::create_notification_channel,
        notification_channels::get_notification_channel,
        notification_channels::update_notification_channel,
        notification_channels::delete_notification_channel,
        sessions::list_sessions,
        pipeline::list_pipeline_outputs
    ),
    components(schemas(
        crate::domain::streamer::Streamer,
        ErrorResponse,
        crate::domain::types::StreamerUrl,
        crate::domain::types::StreamerState,
        crate::domain::config::MergedConfig,
        crate::domain::types::Filter,
        crate::domain::types::FilterType,
        crate::domain::live_session::LiveSession,
        CreateStreamer,
        streamers::UpdateStreamer,
        crate::domain::template_config::TemplateConfig,
        CreateTemplate,
        UpdateTemplate,
        crate::domain::notification_channel::NotificationChannel,
        CreateNotificationChannel,
        UpdateNotificationChannel,
        crate::domain::types::NotificationChannelType,
        crate::domain::types::NotificationChannelSettings,
        StreamerStatus
    ))
)]
struct ApiDoc;

#[derive(Clone)]
pub struct AppState {
    pub config_service: Arc<ConfigService>,
    pub db_service: Arc<DatabaseService>,
}

pub async fn run(
    config_service: Arc<ConfigService>,
    db_service: Arc<DatabaseService>,
) -> anyhow::Result<()> {
    let state = AppState {
        config_service,
        db_service,
    };

    let api_router = Router::new()
        .nest("/streamers", streamers_routes())
        .nest("/templates", templates_routes())
        .nest("/notification_channels", notification_channels_routes())
        .nest("/sessions", sessions_routes())
        .nest("/pipeline", pipeline_routes())
        .nest("/global_config", global_config_routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/", get(index))
        .route("/health", get(health_check))
        .route("/metrics", get(metrics))
        .nest("/api", api_router)
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], 12555));
    info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health_check() -> &'static str {
    "OK"
}

async fn index() -> Html<&'static str> {
    Html(r#"<h1>srec</h1><p>See <a href="/swagger-ui">Swagger UI</a> for API documentation.</p>"#)
}

async fn metrics() -> String {
    use prometheus::{Encoder, TextEncoder};
    let encoder = TextEncoder::new();
    let mut buffer = vec![];
    encoder
        .encode(&prometheus::gather(), &mut buffer)
        .unwrap();
    String::from_utf8(buffer).unwrap()
}