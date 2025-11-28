use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use rust_srec::database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rust_srec=debug,sqlx=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load environment variables
    dotenvy::dotenv().ok();

    // Initialize database
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:srec.db?mode=rwc".to_string());
    
    let pool = database::init_pool(&database_url).await?;
    
    // Run migrations
    database::run_migrations(&pool).await?;

    tracing::info!("rust-srec initialized successfully");

    Ok(())
}
