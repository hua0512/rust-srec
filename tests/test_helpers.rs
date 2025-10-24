use axum::{middleware, Router};
use rust_srec::{
    api::{
        auth::auth_middleware, global_config::global_config_routes,
        notification_channels::notification_channels_routes, pipeline::pipeline_routes,
        sessions::sessions_routes, streamers::streamers_routes, templates::templates_routes,
        AppState,
    },
    config::ConfigService,
    database::DatabaseService,
};
use std::sync::Arc;
use tokio::net::TcpListener;

pub struct TestApp {
    pub address: String,
    pub client: reqwest::Client,
    pub db_service: Arc<DatabaseService>,
}

/// Spawns the application in the background for testing.
pub async fn spawn_app() -> TestApp {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let address = format!("http://{}", addr);

    let db_service = Arc::new(DatabaseService::new("sqlite::memory:").await.unwrap());
    // The sqlx::migrate! macro will look for a `migrations` directory at the root of the crate.
    // If it's not there, this will fail to compile, and we'll need to adjust.
    sqlx::migrate!("../rust-srec/migrations")
        .run(&db_service.pool)
        .await
        .unwrap();

    let config_service = Arc::new(ConfigService::new(db_service.clone()).await.unwrap());

    let state = AppState {
        config_service,
        db_service: db_service.clone(),
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

    let app = Router::new().nest("/api", api_router).with_state(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    TestApp {
        address,
        client,
        db_service,
    }
}