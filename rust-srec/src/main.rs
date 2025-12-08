//! rust-srec - Streaming Recorder Application
//!
//! A production-ready streaming recorder with support for multiple platforms,
//! danmu collection, post-processing pipelines, and notifications.

use std::sync::Arc;

use rust_srec::database;
use rust_srec::services::ServiceContainer;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "rust_srec=trace,tower_http=trace,axum=trace,sqlx=warn,reqwest=trace,mesio=debug".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting rust-srec v{}", env!("CARGO_PKG_VERSION"));

    // Load environment variables
    dotenvy::dotenv().ok();

    // Initialize database
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:srec.db?mode=rwc".to_string());

    info!("Connecting to database: {}", database_url);
    let pool = database::init_pool(&database_url).await?;

    // Run migrations
    info!("Running database migrations...");
    database::run_migrations(&pool).await?;
    info!("Database migrations complete");

    // Create service container
    info!("Initializing services...");
    let container = Arc::new(ServiceContainer::new(pool).await?);

    // Initialize all services
    container.initialize().await?;

    // Start API server
    container.start_api_server().await?;

    // Send startup notification
    let startup_event = rust_srec::notification::NotificationEvent::SystemStartup {
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now(),
    };
    if let Err(e) = container.notification_service().notify(startup_event).await {
        warn!("Failed to send startup notification: {}", e);
    }

    info!("rust-srec started successfully");

    // Wait for shutdown signal
    let container_shutdown = container.clone();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT (Ctrl+C), initiating shutdown...");
        }
        _ = wait_for_sigterm() => {
            info!("Received SIGTERM, initiating shutdown...");
        }
    }

    // Send shutdown notification
    let shutdown_event = rust_srec::notification::NotificationEvent::SystemShutdown {
        reason: "Signal received".to_string(),
        timestamp: chrono::Utc::now(),
    };
    if let Err(e) = container
        .notification_service()
        .notify(shutdown_event)
        .await
    {
        warn!("Failed to send shutdown notification: {}", e);
    }

    // Graceful shutdown
    info!("Shutting down services...");
    if let Err(e) = container_shutdown.shutdown().await {
        error!("Error during shutdown: {}", e);
    }

    info!("rust-srec shutdown complete");
    Ok(())
}

/// Wait for SIGTERM signal (Unix only).
#[cfg(unix)]
async fn wait_for_sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
    sigterm.recv().await;
}

/// Wait for SIGTERM signal (Windows - uses ctrl_c as fallback).
#[cfg(not(unix))]
async fn wait_for_sigterm() {
    // On Windows, we just wait forever since SIGTERM doesn't exist
    // The ctrl_c handler above will catch Ctrl+C
    std::future::pending::<()>().await;
}
