//! Rate limiting for platform API calls.
//!
//! Implements a token bucket algorithm for rate limiting API requests
//! to prevent abuse and respect platform rate limits.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{debug, trace};

/// Configuration for a rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum tokens (burst capacity).
    pub max_tokens: u32,
    /// Tokens added per second.
    pub refill_rate: f64,
    /// Initial tokens.
    pub initial_tokens: u32,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            max_tokens: 10,
            refill_rate: 1.0, // 1 token per second
            initial_tokens: 10,
        }
    }
}

impl RateLimiterConfig {
    /// Create a config for a specific requests-per-second limit.
    pub fn with_rps(rps: f64) -> Result<Self, crate::Error> {
        if !rps.is_finite() || rps <= 0.0 {
            return Err(crate::Error::Other(format!(
                "rate limit must be a positive finite number, got {}",
                rps
            )));
        }

        let max_tokens = (rps * 2.0).ceil().max(1.0) as u32; // Allow burst of 2x, min 1

        Ok(Self {
            max_tokens,
            refill_rate: rps,
            initial_tokens: max_tokens,
        })
    }
}

/// Token bucket rate limiter.
#[derive(Debug)]
pub struct RateLimiter {
    /// Current number of tokens.
    tokens: f64,
    /// Maximum tokens (burst capacity).
    max_tokens: u32,
    /// Tokens added per second.
    refill_rate: f64,
    /// Last refill time.
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            tokens: config.initial_tokens as f64,
            max_tokens: config.max_tokens,
            refill_rate: config.refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to acquire a token.
    ///
    /// Returns `true` if a token was acquired, `false` if rate limited.
    pub fn try_acquire(&mut self) -> bool {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Acquire a token, waiting if necessary.
    ///
    /// Returns the duration waited.
    pub async fn acquire(&mut self) -> Duration {
        let mut total_wait = Duration::ZERO;

        loop {
            self.refill();

            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                return total_wait;
            }

            // Calculate wait time for next token
            let tokens_needed = 1.0 - self.tokens;
            let wait_secs = tokens_needed / self.refill_rate;
            let wait_duration = Duration::from_secs_f64(wait_secs);

            debug!("Rate limited, waiting {:?}", wait_duration);
            tokio::time::sleep(wait_duration).await;
            total_wait += wait_duration;
        }
    }

    /// Get the current number of available tokens.
    pub fn available_tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// Get the time until the next token is available.
    pub fn time_until_available(&mut self) -> Duration {
        self.refill();

        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            let tokens_needed = 1.0 - self.tokens;
            Duration::from_secs_f64(tokens_needed / self.refill_rate)
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let new_tokens = elapsed.as_secs_f64() * self.refill_rate;

        self.tokens = (self.tokens + new_tokens).min(self.max_tokens as f64);
        self.last_refill = now;
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimiterConfig::default())
    }
}

/// Manager for per-platform rate limiters.
#[derive(Debug, Clone)]
pub struct RateLimiterManager {
    /// Rate limiters by platform ID.
    limiters: Arc<Mutex<HashMap<String, PlatformLimiter>>>,
    /// Default configuration for new limiters.
    default_config: RateLimiterConfig,
    /// Platform-specific configurations.
    platform_configs: HashMap<String, RateLimiterConfig>,
}

#[derive(Debug)]
struct PlatformLimiter {
    bucket: RateLimiter,
    /// Only the head blocking caller waits for the next token. The bucket
    /// map remains unlocked during that wait, including for other platforms.
    admission: Arc<Mutex<()>>,
}

impl RateLimiterManager {
    /// Create a new rate limiter manager.
    pub fn new() -> Self {
        Self::with_config(RateLimiterConfig::default())
    }

    /// Create a new rate limiter manager with a default configuration.
    pub fn with_config(default_config: RateLimiterConfig) -> Self {
        Self {
            limiters: Arc::new(Mutex::new(HashMap::new())),
            default_config,
            platform_configs: HashMap::new(),
        }
    }

    /// Set a platform-specific configuration.
    pub fn set_platform_config(&mut self, platform_id: &str, config: RateLimiterConfig) {
        self.platform_configs
            .insert(platform_id.to_string(), config);
    }

    /// Configure a stored platform ID with an existing name-based override.
    pub fn alias_platform_config(&mut self, platform_id: &str, platform_name: &str) {
        if let Some(config) = self.platform_configs.get(platform_name).cloned() {
            self.platform_configs.insert(platform_id.to_owned(), config);
        }
    }

    /// Try to acquire a token for a platform.
    /// This remains opportunistic: it does not join the blocking FIFO queue.
    ///
    /// # Cancel Safety
    ///
    /// This method is cancel-safe. The mutex is only held for the duration of
    /// the synchronous `try_acquire()` call, with no await points while holding
    /// the lock.
    pub async fn try_acquire(&self, platform_id: &str) -> bool {
        let mut limiters = self.limiters.lock().await;
        let limiter = self.get_or_create_limiter(&mut limiters, platform_id);
        limiter.bucket.try_acquire()
    }

    /// Acquire a token for a platform, waiting if necessary.
    ///
    /// Blocking callers enter a FIFO queue for this platform. Only its head
    /// waits for a token; callers for other platforms proceed independently.
    /// Returns the duration waited, including time queued behind another caller.
    ///
    /// # Cancel Safety
    ///
    /// This method is cancel-safe. If the future is dropped before completion:
    /// - No tokens are consumed
    /// - The rate limiter state remains consistent
    /// - Subsequent calls will work correctly
    ///
    /// The per-platform queue holds no token reservation. Dropping its head
    /// releases the next caller; dropping a queued caller removes it from the
    /// queue. The shared bucket lock is always released before sleeping.
    pub async fn acquire(&self, platform_id: &str) -> Duration {
        let admission = {
            let mut limiters = self.limiters.lock().await;
            Arc::clone(
                &self
                    .get_or_create_limiter(&mut limiters, platform_id)
                    .admission,
            )
        };
        let (_turn, mut total_wait) = match admission.try_lock() {
            Ok(turn) => (turn, Duration::ZERO),
            Err(_) => {
                let started = Instant::now();
                let turn = admission.lock().await;
                (turn, started.elapsed())
            }
        };
        let platform_id = platform_id.to_string();

        loop {
            // Check availability and try to acquire with the lock held.
            let wait_duration = {
                let mut limiters = self.limiters.lock().await;
                let limiter = self.get_or_create_limiter(&mut limiters, &platform_id);

                // Try to acquire immediately
                if limiter.bucket.try_acquire() {
                    return total_wait;
                }

                // Calculate wait time for next token
                limiter.bucket.time_until_available()
            }; // Lock released here - CANCEL SAFE POINT

            // Wait without holding the lock.
            // If cancelled here, no state is corrupted
            trace!(
                platform_id = %platform_id,
                wait = ?wait_duration,
                "rate limited"
            );
            tokio::time::sleep(wait_duration).await;
            total_wait += wait_duration;

            // An opportunistic try_acquire may have taken the token.
        }
    }

    /// Get available tokens for a platform.
    pub async fn available_tokens(&self, platform_id: &str) -> f64 {
        let mut limiters = self.limiters.lock().await;
        let limiter = self.get_or_create_limiter(&mut limiters, platform_id);
        limiter.bucket.available_tokens()
    }

    /// Get or create a rate limiter for a platform.
    fn get_or_create_limiter<'a>(
        &self,
        limiters: &'a mut HashMap<String, PlatformLimiter>,
        platform_id: &str,
    ) -> &'a mut PlatformLimiter {
        match limiters.entry(platform_id.to_string()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let config = self
                    .platform_configs
                    .get(platform_id)
                    .cloned()
                    .unwrap_or_else(|| self.default_config.clone());
                entry.insert(PlatformLimiter {
                    bucket: RateLimiter::new(config),
                    admission: Arc::new(Mutex::new(())),
                })
            }
        }
    }
}

impl Default for RateLimiterManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_refill_at_the_rate_up_to_a_burst_of_twice_the_rps() {
        for (rps, burst) in [(5.0, 10), (2.5, 5), (0.2, 1)] {
            let config = RateLimiterConfig::with_rps(rps).unwrap();
            assert_eq!(
                (config.max_tokens, config.initial_tokens, config.refill_rate),
                (burst, burst, rps)
            );
        }

        // The bucket reads `std::time::Instant`, so elapsed time is simulated by
        // moving `last_refill` back. A slow rate keeps the test's own run time
        // from adding a token.
        let mut limiter = RateLimiter::new(RateLimiterConfig {
            max_tokens: 3,
            refill_rate: 0.1,
            initial_tokens: 0,
        });
        let idle = |limiter: &mut RateLimiter, secs| {
            limiter.last_refill = Instant::now()
                .checked_sub(Duration::from_secs(secs))
                .expect("monotonic clock covers the idle period");
        };
        assert!(!limiter.try_acquire());
        idle(&mut limiter, 10);
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());

        idle(&mut limiter, 60);
        assert_eq!(limiter.available_tokens(), 3.0);
        for _ in 0..3 {
            assert!(limiter.try_acquire());
        }
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_rate_limiter_config_with_invalid_rps() {
        assert!(RateLimiterConfig::with_rps(0.0).is_err());
        assert!(RateLimiterConfig::with_rps(-1.0).is_err());
        assert!(RateLimiterConfig::with_rps(f64::NAN).is_err());
        assert!(RateLimiterConfig::with_rps(f64::INFINITY).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn blocking_waiters_are_fifo_and_cancellation_spends_no_tokens() {
        let mut manager = RateLimiterManager::with_config(RateLimiterConfig {
            max_tokens: 1,
            refill_rate: 1.0,
            initial_tokens: 0,
        });
        manager.set_platform_config(
            "other",
            RateLimiterConfig {
                max_tokens: 1,
                refill_rate: 1.0,
                initial_tokens: 1,
            },
        );
        let mut first = Box::pin(manager.acquire("queued"));
        let mut cancelled = Box::pin(manager.acquire("queued"));
        let mut last = Box::pin(manager.acquire("queued"));
        assert!(futures::poll!(first.as_mut()).is_pending());
        assert!(futures::poll!(cancelled.as_mut()).is_pending());
        assert!(futures::poll!(last.as_mut()).is_pending());
        drop(cancelled);

        // A sleeping queue head does not hold the shared bucket lock.
        assert!(manager.try_acquire("other").await);
        {
            let mut limiters = manager.limiters.lock().await;
            limiters.get_mut("queued").unwrap().bucket.tokens = 1.0;
        }
        assert!(futures::poll!(last.as_mut()).is_pending());
        // Advance only the sleep clock; the manually supplied token avoids
        // coupling FIFO/cancellation assertions to a real-time refill race.
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(futures::poll!(first.as_mut()).is_ready());
        assert!(futures::poll!(last.as_mut()).is_pending());
        assert!(!manager.try_acquire("queued").await);

        // The remaining caller is now the sleeping head. Cancelling it must
        // release the turn without reserving a future token.
        drop(last);
        {
            let mut limiters = manager.limiters.lock().await;
            limiters.get_mut("queued").unwrap().bucket.tokens = 1.0;
        }
        assert_eq!(manager.acquire("queued").await, Duration::ZERO);
        assert!(!manager.try_acquire("queued").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_manager_concurrent_acquire() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let manager = Arc::new(RateLimiterManager::with_config(RateLimiterConfig {
            max_tokens: 5,
            refill_rate: 100.0, // Fast refill for testing
            initial_tokens: 5,
        }));

        let success_count = Arc::new(AtomicU32::new(0));
        let mut handles = vec![];

        // Spawn 10 concurrent acquire tasks
        for _ in 0..10 {
            let manager_clone = manager.clone();
            let success_clone = success_count.clone();
            handles.push(tokio::spawn(async move {
                let _ = manager_clone.acquire("concurrent").await;
                success_clone.fetch_add(1, Ordering::SeqCst);
            }));
        }

        // Wait for all with timeout to detect deadlocks
        let result =
            tokio::time::timeout(Duration::from_secs(2), futures::future::join_all(handles)).await;

        assert!(result.is_ok(), "Concurrent acquires should not deadlock");
        assert_eq!(
            success_count.load(Ordering::SeqCst),
            10,
            "All acquires should complete"
        );
    }
}
