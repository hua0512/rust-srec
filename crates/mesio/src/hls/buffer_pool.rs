// HLS Buffer Pool: Provides reusable buffers to reduce allocation overhead.
//
// This module implements a thread-safe buffer pool for segment processing,
// reducing memory allocation pressure during high-throughput streaming.

use crate::hls::config::BufferPoolConfig;
use crate::hls::metrics::PerformanceMetrics;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Statistics for buffer pool operations
#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    /// Number of new buffer allocations
    pub allocations: u64,
    /// Number of buffer reuses from pool
    pub reuses: u64,
    /// Current number of buffers in the pool
    pub current_pool_size: usize,
}

/// Thread-safe buffer pool for segment processing
///
/// Provides reusable `Vec<u8>` buffers to reduce allocation overhead during
/// segment decryption and processing. Buffers are cleared before reuse to
/// ensure sensitive data is not leaked.
pub struct BufferPool {
    config: BufferPoolConfig,
    pool: Mutex<Vec<Vec<u8>>>,
    /// Number of new buffer allocations
    allocations: AtomicU64,
    /// Number of buffer reuses from pool
    reuses: AtomicU64,
    /// Optional shared performance metrics for recording allocations/reuses
    metrics: Option<Arc<PerformanceMetrics>>,
}

impl BufferPool {
    /// Create a new BufferPool with the given configuration
    pub fn new(config: BufferPoolConfig) -> Self {
        Self {
            config,
            pool: Mutex::new(Vec::new()),
            allocations: AtomicU64::new(0),
            reuses: AtomicU64::new(0),
            metrics: None,
        }
    }

    /// Create a new BufferPool with the given configuration and shared metrics
    pub fn with_metrics(config: BufferPoolConfig, metrics: Arc<PerformanceMetrics>) -> Self {
        Self {
            config,
            pool: Mutex::new(Vec::new()),
            allocations: AtomicU64::new(0),
            reuses: AtomicU64::new(0),
            metrics: Some(metrics),
        }
    }

    /// Set the shared performance metrics for this buffer pool
    pub fn set_metrics(&mut self, metrics: Arc<PerformanceMetrics>) {
        self.metrics = Some(metrics);
    }

    /// Check if buffer pooling is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Acquire a buffer from the pool or allocate a new one
    ///
    /// If pooling is disabled or the pool is empty, a new buffer is allocated.
    /// The returned buffer is guaranteed to have at least `min_capacity` bytes
    /// of capacity.
    ///
    /// # Arguments
    /// * `min_capacity` - Minimum required capacity for the buffer
    ///
    /// # Returns
    /// A `Vec<u8>` with at least `min_capacity` bytes of capacity
    pub fn acquire(&self, min_capacity: usize) -> Vec<u8> {
        if !self.config.enabled {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            // Record to shared metrics if available
            if let Some(ref metrics) = self.metrics {
                metrics.record_buffer_allocation();
            }
            return Vec::with_capacity(min_capacity.max(self.config.default_capacity));
        }

        let mut pool = self.pool.lock().unwrap();

        // Try to find a buffer with sufficient capacity
        if let Some(pos) = pool.iter().position(|buf| buf.capacity() >= min_capacity) {
            let buffer = pool.swap_remove(pos);
            self.reuses.fetch_add(1, Ordering::Relaxed);
            // Record reuse to shared metrics if available
            if let Some(ref metrics) = self.metrics {
                metrics.record_buffer_reuse();
            }
            return buffer;
        }

        // No suitable buffer found, allocate new
        drop(pool); // Release lock before allocation
        self.allocations.fetch_add(1, Ordering::Relaxed);
        // Record allocation to shared metrics if available
        if let Some(ref metrics) = self.metrics {
            metrics.record_buffer_allocation();
        }
        debug!(
            min_capacity = min_capacity,
            default_capacity = self.config.default_capacity,
            "Buffer pool exhausted, allocating new buffer"
        );
        Vec::with_capacity(min_capacity.max(self.config.default_capacity))
    }

    /// Return a buffer to the pool
    ///
    /// The buffer is cleared (all bytes zeroed) before being added to the pool
    /// to ensure sensitive data is not leaked. If the pool is at capacity,
    /// the buffer is dropped.
    ///
    /// # Arguments
    /// * `buffer` - The buffer to return to the pool
    pub fn release(&self, mut buffer: Vec<u8>) {
        if !self.config.enabled {
            return;
        }

        // Clear sensitive data by zeroing the buffer
        // We need to fill the entire capacity, not just the length
        let capacity = buffer.capacity();
        buffer.clear();
        buffer.resize(capacity, 0);
        buffer.clear();

        let mut pool = self.pool.lock().unwrap();

        // Only keep buffer if pool is not at capacity
        if pool.len() < self.config.pool_size {
            pool.push(buffer);
        }
        // Otherwise, buffer is dropped
    }

    /// Get current buffer pool statistics
    pub fn stats(&self) -> BufferPoolStats {
        let pool = self.pool.lock().unwrap();
        BufferPoolStats {
            allocations: self.allocations.load(Ordering::Relaxed),
            reuses: self.reuses.load(Ordering::Relaxed),
            current_pool_size: pool.len(),
        }
    }

    /// Get the number of allocations
    pub fn allocations(&self) -> u64 {
        self.allocations.load(Ordering::Relaxed)
    }

    /// Get the number of reuses
    pub fn reuses(&self) -> u64 {
        self.reuses.load(Ordering::Relaxed)
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(BufferPoolConfig::default())
    }
}

/// Synchronize buffer pool stats to shared performance metrics
///
/// This is useful when you want to update the shared metrics with the
/// current buffer pool statistics at a specific point in time.
impl BufferPool {
    /// Sync current buffer pool stats to the shared performance metrics
    ///
    /// This method is useful for batch updates or when you want to ensure
    /// the shared metrics reflect the current state of the buffer pool.
    pub fn sync_to_metrics(&self) {
        if self.metrics.is_some() {
            // The metrics are already being updated in real-time via acquire(),
            // but this method can be used for any additional synchronization needs
            let stats = self.stats();
            debug!(
                allocations = stats.allocations,
                reuses = stats.reuses,
                pool_size = stats.current_pool_size,
                "Buffer pool stats synced to metrics"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_buffer_pool_basic() {
        let config = BufferPoolConfig {
            enabled: true,
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::new(config);

        // Acquire a buffer
        let buffer = pool.acquire(512);
        assert!(buffer.capacity() >= 512);

        // Stats should show one allocation
        let stats = pool.stats();
        assert_eq!(stats.allocations, 1);
        assert_eq!(stats.reuses, 0);
        assert_eq!(stats.current_pool_size, 0);
    }

    #[test]
    fn test_buffer_pool_reuse() {
        let config = BufferPoolConfig {
            enabled: true,
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::new(config);

        // Acquire and release a buffer
        let buffer = pool.acquire(512);
        pool.release(buffer);

        // Pool should have one buffer
        assert_eq!(pool.stats().current_pool_size, 1);

        // Acquire again - should reuse
        let _buffer2 = pool.acquire(512);
        let stats = pool.stats();
        assert_eq!(stats.allocations, 1);
        assert_eq!(stats.reuses, 1);
        assert_eq!(stats.current_pool_size, 0);
    }

    #[test]
    fn test_buffer_pool_disabled() {
        let config = BufferPoolConfig {
            enabled: false,
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::new(config);

        // Acquire a buffer
        let buffer = pool.acquire(512);
        assert!(buffer.capacity() >= 512);

        // Release should not add to pool
        pool.release(buffer);
        assert_eq!(pool.stats().current_pool_size, 0);

        // Acquire again - should allocate new
        let _buffer2 = pool.acquire(512);
        let stats = pool.stats();
        assert_eq!(stats.allocations, 2);
        assert_eq!(stats.reuses, 0);
    }

    #[test]
    fn test_buffer_pool_capacity_limit() {
        let config = BufferPoolConfig {
            enabled: true,
            pool_size: 2,
            default_capacity: 1024,
        };
        let pool = BufferPool::new(config);

        // Acquire and release 3 buffers
        let b1 = pool.acquire(512);
        let b2 = pool.acquire(512);
        let b3 = pool.acquire(512);

        pool.release(b1);
        pool.release(b2);
        pool.release(b3); // This one should be dropped

        // Pool should only have 2 buffers
        assert_eq!(pool.stats().current_pool_size, 2);
    }

    #[test]
    fn test_buffer_pool_min_capacity() {
        let config = BufferPoolConfig {
            enabled: true,
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::new(config);

        // Acquire a small buffer and release
        let small = pool.acquire(256);
        pool.release(small);

        // Acquire a larger buffer - should allocate new since pool buffer is too small
        let large = pool.acquire(2048);
        assert!(large.capacity() >= 2048);

        let stats = pool.stats();
        assert_eq!(stats.allocations, 2);
        assert_eq!(stats.reuses, 0);
    }

    #[test]
    fn test_buffer_pool_metrics_integration() {
        use crate::hls::metrics::PerformanceMetrics;
        use std::sync::atomic::Ordering;

        let metrics = Arc::new(PerformanceMetrics::new());
        let config = BufferPoolConfig {
            enabled: true,
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::with_metrics(config, Arc::clone(&metrics));

        // Initial state - no allocations or reuses
        assert_eq!(metrics.buffer_allocations.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.buffer_reuses.load(Ordering::Relaxed), 0);

        // Acquire a buffer - should record allocation to metrics
        let buffer = pool.acquire(512);
        assert_eq!(metrics.buffer_allocations.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.buffer_reuses.load(Ordering::Relaxed), 0);

        // Release the buffer
        pool.release(buffer);

        // Acquire again - should record reuse to metrics
        let _buffer2 = pool.acquire(512);
        assert_eq!(metrics.buffer_allocations.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.buffer_reuses.load(Ordering::Relaxed), 1);

        // Verify local stats match metrics
        let stats = pool.stats();
        assert_eq!(
            stats.allocations,
            metrics.buffer_allocations.load(Ordering::Relaxed)
        );
        assert_eq!(stats.reuses, metrics.buffer_reuses.load(Ordering::Relaxed));
    }

    #[test]
    fn test_buffer_pool_metrics_disabled_pool() {
        use crate::hls::metrics::PerformanceMetrics;
        use std::sync::atomic::Ordering;

        let metrics = Arc::new(PerformanceMetrics::new());
        let config = BufferPoolConfig {
            enabled: false, // Pool disabled
            pool_size: 5,
            default_capacity: 1024,
        };
        let pool = BufferPool::with_metrics(config, Arc::clone(&metrics));

        // Acquire buffers - should still record allocations to metrics
        let buffer1 = pool.acquire(512);
        let buffer2 = pool.acquire(512);

        // Both should be allocations (no reuse when pool is disabled)
        assert_eq!(metrics.buffer_allocations.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.buffer_reuses.load(Ordering::Relaxed), 0);

        // Release doesn't add to pool when disabled
        pool.release(buffer1);
        pool.release(buffer2);

        // Acquire again - still allocations
        let _buffer3 = pool.acquire(512);
        assert_eq!(metrics.buffer_allocations.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.buffer_reuses.load(Ordering::Relaxed), 0);
    }
}
