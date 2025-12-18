//! rust-srec - Streaming Recorder Application
//!
//! A production-ready streaming recorder with support for multiple platforms,
//! danmu collection, post-processing pipelines, and notifications.

use std::sync::Arc;

use rust_srec::database;
use rust_srec::logging::init_logging;
use rust_srec::services::ServiceContainer;
use tracing::{error, info, warn};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging with reloadable filter
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "logs".to_string());
    let (logging_config, _guard) = init_logging(&log_dir)
        .map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;

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

    // Apply persisted log filter from database
    logging_config
        .apply_persisted_filter(&container.config_service)
        .await;

    // Start log retention cleanup task
    logging_config.start_retention_cleanup(container.cancellation_token());

    // Store logging config in container for API access
    container.set_logging_config(logging_config.clone());

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
