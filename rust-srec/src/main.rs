use anyhow::Result;
use dotenvy::dotenv;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // 1. Initialize services
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    // let (service, notification_service) = Service::new(&database_url).await?;
    // let config_service = service.config_service.clone();
    // println!("Configurations loaded successfully.");

    // // Subscribe to config updates for verification
    // let mut rx = config_service.subscribe();
    // tokio::spawn(async move {
    //     while let Ok(event) = rx.recv().await {
    //         println!("Received config update event: {:?}", event);
    //     }
    // });

    // let mut scheduler = Scheduler::new(
    //     config_service.clone(),
    //     service.db_service.clone(),
    //     service.download_manager.clone(),
    //     service.danmu_service.clone(),
    //     service.event_broadcaster.clone(),
    // );

    // println!("Application started. Scheduler is running.");

    // // 2. Run the main scheduler loop
    // tokio::spawn(async move {
    //     scheduler.run().await;
    // });

    // // 3. Start the notification service
    // tokio::spawn(async move {
    //     notification_service.run().await;
    // });

    // // 4. Start the API server
    // tokio::spawn(async move {
    //     if let Err(e) = api::run_server(config_service).await {
    //         eprintln!("API server error: {}", e);
    //     }
    // });

    // Keep the main thread alive
    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");

    Ok(())
}
