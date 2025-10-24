pub mod providers;
use crate::domain::streamer::Streamer;
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::Mutex as AsyncMutex;
use std::time::SystemTime;

/// Represents a single Danmu message.
#[derive(Debug, Clone)]
pub struct DanmuMessage {
    pub user_id: String,
    pub nickname: String,
    pub content: String,
    pub timestamp: SystemTime,
}

/// Trait for platform-specific Danmu providers.
#[async_trait::async_trait]
pub trait DanmuProvider {
    async fn connect(&mut self) -> Result<(), anyhow::Error>;
    async fn next_message(&mut self) -> Option<Result<DanmuMessage, anyhow::Error>>;
    async fn close(&mut self) -> Result<(), anyhow::Error>;
}

/// Service for collecting Danmu messages.
pub struct DanmuService {
    provider: Arc<AsyncMutex<dyn DanmuProvider + Send>>,
    stop_signal: Arc<AtomicBool>,
    messages: Arc<Mutex<Vec<DanmuMessage>>>,
}

impl DanmuService {
    /// Creates a new DanmuService.
    pub fn new(streamer: &Streamer) -> Result<Self> {
        let provider: Arc<AsyncMutex<dyn DanmuProvider + Send>> = match streamer.platform.as_str() {
            "bilibili" => Arc::new(AsyncMutex::new(
                providers::bilibili::BilibiliDanmuProvider::new(&streamer.url.0)?,
            )),
            _ => {
                return Err(anyhow::anyhow!(
                    "Danmu provider not found for platform: {}",
                    streamer.platform
                ))
            }
        };

        Ok(Self {
            provider,
            stop_signal: Arc::new(AtomicBool::new(false)),
            messages: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Starts the Danmu collection process.
    pub fn start_collection(&self) {
        let provider_clone = self.provider.clone();
        let stop_signal = self.stop_signal.clone();
        let messages = self.messages.clone();

        tokio::spawn(async move {
            if let Err(e) = provider_clone.lock().await.connect().await {
                // TODO: Add proper error handling
                eprintln!("Failed to connect: {}", e);
                return;
            }

            while !stop_signal.load(Ordering::SeqCst) {
                let next_message = provider_clone.lock().await.next_message().await;
                if let Some(Ok(message)) = next_message {
                    messages.lock().unwrap().push(message);
                }
            }

            if let Err(e) = provider_clone.lock().await.close().await {
                eprintln!("Failed to close connection: {}", e);
            }
        });
    }

    /// Stops the Danmu collection process.
    pub fn stop_collection(&self) -> Result<()> {
        self.stop_signal.store(true, Ordering::SeqCst);

        // TODO: Implement XML serialization
        // TODO: Implement DanmuStatistics calculation

        Ok(())
    }
}
