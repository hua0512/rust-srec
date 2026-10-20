//! Retry policy value object.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Retry policy for transient errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial delay between retries in milliseconds.
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Maximum delay between retries in milliseconds.
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
    /// Backoff multiplier for exponential backoff.
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays.
    #[serde(default = "default_true")]
    pub use_jitter: bool,
}

fn default_max_retries() -> u32 {
    3
}

fn default_initial_delay_ms() -> u64 {
    1000
}

fn default_max_delay_ms() -> u64 {
    30000
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

fn default_true() -> bool {
    true
}

impl RetryPolicy {
    /// Calculate the delay for a given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt >= self.max_retries {
            return Duration::from_millis(self.max_delay_ms);
        }

        let base_delay =
            self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);

        let delay_ms = base_delay.min(self.max_delay_ms as f64) as u64;

        if self.use_jitter {
            // Add up to 25% jitter
            let jitter = (delay_ms as f64 * 0.25 * rand::random::<f64>()) as u64;
            Duration::from_millis(delay_ms + jitter)
        } else {
            Duration::from_millis(delay_ms)
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_delay_ms: default_initial_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            backoff_multiplier: default_backoff_multiplier(),
            use_jitter: default_true(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_calculation_no_jitter() {
        let policy = RetryPolicy {
            max_retries: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            use_jitter: false,
        };

        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(1000));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(2000));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(4000));
        assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(8000));
        assert_eq!(policy.delay_for_attempt(4), Duration::from_millis(16000));
    }

    #[test]
    fn test_delay_capped_at_max() {
        let policy = RetryPolicy {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            backoff_multiplier: 2.0,
            use_jitter: false,
        };

        // After a few attempts, should be capped at max
        assert_eq!(policy.delay_for_attempt(5), Duration::from_millis(5000));
    }
}
