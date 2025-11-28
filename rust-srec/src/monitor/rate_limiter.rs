//! Rate limiting for platform API calls.
//!
//! Implements a token bucket algorithm for rate limiting API requests
//! to prevent abuse and respect platform rate limits.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::debug;

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
    pub fn with_rps(rps: f64) -> Self {
        Self {
            max_tokens: (rps * 2.0) as u32, // Allow burst of 2x
            refill_rate: rps,
            initial_tokens: (rps * 2.0) as u32,
        }
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
    limiters: Arc<Mutex<HashMap<String, RateLimiter>>>,
    /// Default configuration for new limiters.
    default_config: RateLimiterConfig,
    /// Platform-specific configurations.
    platform_configs: HashMap<String, RateLimiterConfig>,
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
        self.platform_configs.insert(platform_id.to_string(), config);
    }

    /// Try to acquire a token for a platform.
    pub async fn try_acquire(&self, platform_id: &str) -> bool {
        let mut limiters = self.limiters.lock().await;
        let limiter = self.get_or_create_limiter(&mut limiters, platform_id);
        limiter.try_acquire()
    }

    /// Acquire a token for a platform, waiting if necessary.
    pub async fn acquire(&self, platform_id: &str) -> Duration {
        let mut limiters = self.limiters.lock().await;
        let limiter = self.get_or_create_limiter(&mut limiters, platform_id);
        limiter.acquire().await
    }

    /// Get available tokens for a platform.
    pub async fn available_tokens(&self, platform_id: &str) -> f64 {
        let mut limiters = self.limiters.lock().await;
        let limiter = self.get_or_create_limiter(&mut limiters, platform_id);
        limiter.available_tokens()
    }

    /// Get or create a rate limiter for a platform.
    fn get_or_create_limiter<'a>(
        &self,
        limiters: &'a mut HashMap<String, RateLimiter>,
        platform_id: &str,
    ) -> &'a mut RateLimiter {
        if !limiters.contains_key(platform_id) {
            let config = self
                .platform_configs
                .get(platform_id)
                .cloned()
                .unwrap_or_else(|| self.default_config.clone());
            limiters.insert(platform_id.to_string(), RateLimiter::new(config));
        }
        limiters.get_mut(platform_id).unwrap()
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
    fn test_rate_limiter_try_acquire() {
        let config = RateLimiterConfig {
            max_tokens: 2,
            refill_rate: 1.0,
            initial_tokens: 2,
        };
        let mut limiter = RateLimiter::new(config);

        // Should be able to acquire 2 tokens
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());

        // Third should fail
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_rate_limiter_refill() {
        let config = RateLimiterConfig {
            max_tokens: 10,
            refill_rate: 100.0, // Fast refill for testing
            initial_tokens: 0,
        };
        let mut limiter = RateLimiter::new(config);

        // Initially no tokens
        assert!(!limiter.try_acquire());

        // Wait a bit for refill
        std::thread::sleep(Duration::from_millis(20));

        // Should have tokens now
        assert!(limiter.try_acquire());
    }

    #[test]
    fn test_rate_limiter_config_with_rps() {
        let config = RateLimiterConfig::with_rps(5.0);
        assert_eq!(config.max_tokens, 10); // 2x burst
        assert_eq!(config.refill_rate, 5.0);
    }

    #[tokio::test]
    async fn test_rate_limiter_manager() {
        let manager = RateLimiterManager::new();

        // First acquire should succeed
        assert!(manager.try_acquire("twitch").await);

        // Check available tokens
        let tokens = manager.available_tokens("twitch").await;
        assert!(tokens < 10.0); // Should have used one token
    }
}
