//! Write batching for database operations.
//!
//! This module provides a generic batch writer that buffers writes and flushes
//! them periodically or when the buffer reaches a certain size.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

/// Configuration for the batch writer.
#[derive(Debug, Clone)]
pub struct BatchWriterConfig {
    /// Maximum number of items to buffer before flushing.
    pub max_buffer_size: usize,
    /// Maximum time to wait before flushing.
    pub flush_interval: Duration,
}

impl Default for BatchWriterConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 100,
            flush_interval: Duration::from_secs(5),
        }
    }
}

/// A generic batch writer that buffers items and flushes them periodically.
pub struct BatchWriter<T> {
    sender: mpsc::Sender<T>,
    _handle: tokio::task::JoinHandle<()>,
}

impl<T: Send + 'static> BatchWriter<T> {
    /// Create a new batch writer with the given configuration and flush function.
    pub fn new<F, Fut>(config: BatchWriterConfig, flush_fn: F) -> Self
    where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), crate::Error>> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<T>(config.max_buffer_size * 2);
        let flush_fn = Arc::new(flush_fn);

        let handle = tokio::spawn(Self::run_flush_loop(receiver, config, flush_fn));

        Self {
            sender,
            _handle: handle,
        }
    }

    /// Add an item to the batch.
    pub async fn add(&self, item: T) -> Result<(), crate::Error> {
        self.sender
            .send(item)
            .await
            .map_err(|_| crate::Error::Other("Batch writer channel closed".to_string()))
    }

    async fn run_flush_loop<F, Fut>(
        mut receiver: mpsc::Receiver<T>,
        config: BatchWriterConfig,
        flush_fn: Arc<F>,
    ) where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), crate::Error>> + Send + 'static,
    {
        let mut buffer = Vec::with_capacity(config.max_buffer_size);
        let mut flush_timer = interval(config.flush_interval);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // Receive new items
                item = receiver.recv() => {
                    match item {
                        Some(item) => {
                            buffer.push(item);

                            // Flush if buffer is full
                            if buffer.len() >= config.max_buffer_size {
                                if let Err(e) = flush_fn(std::mem::take(&mut buffer)).await {
                                    tracing::error!("Batch flush error: {}", e);
                                }
                                buffer = Vec::with_capacity(config.max_buffer_size);
                            }
                        }
                        None => {
                            // Channel closed, flush remaining items
                            if !buffer.is_empty() && let Err(e) = flush_fn(buffer).await {
                                tracing::error!("Final batch flush error: {}", e);
                            }
                            break;
                        }
                    }
                }

                // Periodic flush
                _ = flush_timer.tick() => {
                    if !buffer.is_empty() {
                        if let Err(e) = flush_fn(std::mem::take(&mut buffer)).await {
                            tracing::error!("Periodic batch flush error: {}", e);
                        }
                        buffer = Vec::with_capacity(config.max_buffer_size);
                    }
                }
            }
        }
    }
}

/// Statistics update for batch writing.
#[derive(Debug, Clone)]
pub struct StatsUpdate {
    pub streamer_id: String,
    pub bytes_downloaded: i64,
    pub segments_completed: i32,
}

/// Job status update for batch writing.
#[derive(Debug, Clone)]
pub struct JobStatusUpdate {
    pub job_id: String,
    pub status: String,
    pub state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_batch_writer_flush_on_size() {
        let flush_count = Arc::new(AtomicUsize::new(0));
        let flush_count_clone = flush_count.clone();

        let config = BatchWriterConfig {
            max_buffer_size: 3,
            flush_interval: Duration::from_secs(60), // Long interval to test size-based flush
        };

        let writer = BatchWriter::new(config, move |items: Vec<i32>| {
            let count = flush_count_clone.clone();
            async move {
                count.fetch_add(items.len(), Ordering::SeqCst);
                Ok(())
            }
        });

        // Add 3 items to trigger flush
        writer.add(1).await.unwrap();
        writer.add(2).await.unwrap();
        writer.add(3).await.unwrap();

        // Give time for flush
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(flush_count.load(Ordering::SeqCst), 3);
    }
}
