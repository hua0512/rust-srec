//! Write batching for database operations.
//!
//! This module provides a generic batch writer that buffers writes and flushes
//! them periodically or when the buffer reaches a certain size.
//!
//! # Usage
//!
//! ```ignore
//! use rust_srec::database::batching::{BatchWriter, BatchWriterConfig, StatsUpdate};
//!
//! let config = BatchWriterConfig::default();
//! let writer = BatchWriter::new(config, |updates: Vec<StatsUpdate>| async move {
//!     // Batch insert/update to database
//!     Ok(())
//! });
//!
//! writer.add(StatsUpdate {
//!     streamer_id: "streamer-1".to_string(),
//!     bytes_downloaded: 1024,
//!     segments_completed: 1,
//! }).await?;
//! ```

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::Instant;
use tokio::time::interval;

/// Maximum number of failed batches retained before producers are backpressured.
const RETAINED_BATCH_MULTIPLIER: usize = 10;

struct RetainedItem<T> {
    value: T,
    _permit: OwnedSemaphorePermit,
}

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
    sender: mpsc::Sender<RetainedItem<T>>,
    retention_permits: Arc<Semaphore>,
    _handle: tokio::task::JoinHandle<()>,
}

impl<T: Send + Clone + 'static> BatchWriter<T> {
    /// Create a new batch writer with the given configuration and flush function.
    pub fn new<F, Fut>(mut config: BatchWriterConfig, flush_fn: F) -> Self
    where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), crate::Error>> + Send + 'static,
    {
        let requested_buffer_size = config.max_buffer_size;
        config.max_buffer_size =
            requested_buffer_size.clamp(1, Semaphore::MAX_PERMITS / RETAINED_BATCH_MULTIPLIER);
        if config.max_buffer_size != requested_buffer_size {
            tracing::warn!(
                requested = requested_buffer_size,
                effective = config.max_buffer_size,
                "Batch writer buffer size was outside the supported range"
            );
        }
        let channel_capacity = config.max_buffer_size.saturating_mul(2);
        let retained_capacity = config
            .max_buffer_size
            .saturating_mul(RETAINED_BATCH_MULTIPLIER)
            .min(Semaphore::MAX_PERMITS);
        let retention_permits = Arc::new(Semaphore::new(retained_capacity));
        let (sender, receiver) = mpsc::channel(channel_capacity);
        let flush_fn = Arc::new(flush_fn);

        let handle = tokio::spawn(Self::run_flush_loop(receiver, config, flush_fn));

        Self {
            sender,
            retention_permits,
            _handle: handle,
        }
    }

    /// Add an item to the batch.
    pub async fn add(&self, item: T) -> Result<(), crate::Error> {
        let retention_permits = self.retention_permits.clone();
        let permit = tokio::select! {
            permit = retention_permits.acquire_owned() => permit
                .map_err(|_| crate::Error::Other("Batch writer closed".to_string()))?,
            _ = self.sender.closed() => {
                return Err(crate::Error::Other("Batch writer channel closed".to_string()));
            }
        };
        self.sender
            .send(RetainedItem {
                value: item,
                _permit: permit,
            })
            .await
            .map_err(|_| crate::Error::Other("Batch writer channel closed".to_string()))
    }

    async fn flush_buffer<F, Fut>(
        buffer: &mut Vec<RetainedItem<T>>,
        flush_fn: &F,
    ) -> Result<(), crate::Error>
    where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), crate::Error>> + Send + 'static,
    {
        let batch = buffer.iter().map(|item| item.value.clone()).collect();
        flush_fn(batch).await?;
        buffer.clear();
        Ok(())
    }

    async fn run_flush_loop<F, Fut>(
        mut receiver: mpsc::Receiver<RetainedItem<T>>,
        config: BatchWriterConfig,
        flush_fn: Arc<F>,
    ) where
        T: Clone,
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), crate::Error>> + Send + 'static,
    {
        let mut buffer = Vec::with_capacity(config.max_buffer_size);
        let mut flush_timer = interval(config.flush_interval);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut backoff = Duration::ZERO;
        let mut next_flush_allowed = Instant::now();

        loop {
            tokio::select! {
                // Receive new items
                item = receiver.recv() => {
                    match item {
                        Some(item) => {
                            buffer.push(item);

                            // Flush if buffer is full
                            if buffer.len() >= config.max_buffer_size {
                                if Instant::now() >= next_flush_allowed {
                                    if let Err(e) = Self::flush_buffer(&mut buffer, flush_fn.as_ref()).await {
                                        tracing::error!("Batch flush error: {}", e);
                                        backoff = if backoff.is_zero() {
                                            Duration::from_millis(200)
                                        } else {
                                            (backoff * 2).min(Duration::from_secs(5))
                                        };
                                        next_flush_allowed = Instant::now() + backoff;
                                    } else {
                                        backoff = Duration::ZERO;
                                        next_flush_allowed = Instant::now();
                                    }
                                }
                                if buffer.capacity() < config.max_buffer_size {
                                    buffer = Vec::with_capacity(config.max_buffer_size);
                                }
                            }
                        }
                        None => {
                            // Channel closed, flush remaining items
                            if !buffer.is_empty()
                                && let Err(e) = Self::flush_buffer(&mut buffer, flush_fn.as_ref()).await
                            {
                                tracing::error!("Final batch flush error: {}", e);
                            }
                            break;
                        }
                    }
                }

                // Periodic flush
                _ = flush_timer.tick() => {
                    if !buffer.is_empty() && Instant::now() >= next_flush_allowed {
                        if let Err(e) = Self::flush_buffer(&mut buffer, flush_fn.as_ref()).await {
                            tracing::error!("Periodic batch flush error: {}", e);
                            backoff = if backoff.is_zero() {
                                Duration::from_millis(200)
                            } else {
                                (backoff * 2).min(Duration::from_secs(5))
                            };
                            next_flush_allowed = Instant::now() + backoff;
                        } else {
                            backoff = Duration::ZERO;
                            next_flush_allowed = Instant::now();
                        }
                        if buffer.capacity() < config.max_buffer_size {
                            buffer = Vec::with_capacity(config.max_buffer_size);
                        }
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

    #[tokio::test]
    async fn test_batch_writer_requeues_on_error() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let config = BatchWriterConfig {
            max_buffer_size: 2,
            flush_interval: Duration::from_millis(50),
        };

        let writer = BatchWriter::new(config, move |items: Vec<i32>| {
            let attempts = attempts_clone.clone();
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    // Fail first flush, items should be requeued.
                    Err(crate::Error::Other("flush failed".to_string()))
                } else {
                    assert_eq!(items.len(), 2);
                    Ok(())
                }
            }
        });

        writer.add(1).await.unwrap();
        writer.add(2).await.unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if attempts.load(Ordering::SeqCst) >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;

        assert!(result.is_ok(), "Expected at least 2 flush attempts");
    }

    #[tokio::test]
    async fn test_batch_writer_backpressures_when_flush_keeps_failing() {
        let config = BatchWriterConfig {
            max_buffer_size: 1,
            flush_interval: Duration::from_secs(60),
        };
        let writer = BatchWriter::new(config, |_items: Vec<i32>| async {
            Err(crate::Error::Other("flush failed".to_string()))
        });

        for item in 0..RETAINED_BATCH_MULTIPLIER {
            tokio::time::timeout(Duration::from_secs(1), writer.add(item as i32))
                .await
                .unwrap()
                .unwrap();
        }

        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            writer.add(RETAINED_BATCH_MULTIPLIER as i32),
        )
        .await;
        assert!(
            blocked.is_err(),
            "producer should wait at the retention limit"
        );
    }
}
