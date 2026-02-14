// HLS Retry Utility: Shared retry-with-backoff logic for segment and key fetching.
//
// Implements exponential backoff with jitter, max delay cap, and smart error classification

use crate::hls::HlsDownloaderError;
use rand::RngExt;
use std::future::Future;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Base delay between retries. Actual delay = base * 2^attempt + jitter.
    pub base_delay: Duration,
    /// Hard cap on the computed delay to prevent unbounded growth.
    pub max_delay: Duration,
    /// When true, adds random jitter of [0, base_delay/2) to prevent thundering herd.
    pub jitter: bool,
}

impl RetryPolicy {
    /// Compute the delay for a given attempt number (0-indexed).
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        // Avoid `Duration` overflow and keep this O(1) even for misconfigured `attempt`.
        // 2^attempt is computed with a checked shift so attempts >= 32 saturate.
        let multiplier = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        let exp_delay = self
            .base_delay
            .checked_mul(multiplier)
            .unwrap_or(self.max_delay);
        let capped = exp_delay.min(self.max_delay);

        if !self.jitter {
            return capped;
        }

        // Jitter is limited so the final delay never exceeds `max_delay`.
        let jitter_range_ms = u64::try_from(self.base_delay.as_millis()).unwrap_or(u64::MAX) / 2;
        if jitter_range_ms == 0 {
            return capped;
        }

        let remaining_ms =
            u64::try_from(self.max_delay.saturating_sub(capped).as_millis()).unwrap_or(0);
        let jitter_limit_ms = jitter_range_ms.min(remaining_ms);
        if jitter_limit_ms == 0 {
            return capped;
        }

        let jitter_ms = rand::rng().random_range(0..jitter_limit_ms);
        (capped + Duration::from_millis(jitter_ms)).min(self.max_delay)
    }
}

/// Result of a single attempt, used by the caller to signal retryability.
pub enum RetryAction<T> {
    /// Operation succeeded.
    Success(T),
    /// Operation failed with a retryable error (network, 5xx, timeout).
    Retry(HlsDownloaderError),
    /// Operation failed with a non-retryable error (4xx, parse error).
    Fail(HlsDownloaderError),
}

/// Execute an async operation with retry-and-backoff.
///
/// The `operation` closure receives the current attempt number (0-indexed) and
/// returns a [`RetryAction`] indicating whether the result is a success,
/// retryable failure, or permanent failure.
pub async fn retry_with_backoff<F, Fut, T>(
    policy: &RetryPolicy,
    token: &CancellationToken,
    operation: F,
) -> Result<T, HlsDownloaderError>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = RetryAction<T>>,
{
    for attempt in 0..=policy.max_retries {
        if token.is_cancelled() {
            return Err(HlsDownloaderError::Cancelled);
        }

        match operation(attempt).await {
            RetryAction::Success(value) => return Ok(value),
            RetryAction::Fail(err) => return Err(err),
            RetryAction::Retry(err) => {
                if attempt >= policy.max_retries {
                    return Err(err);
                }
                let delay = policy.delay_for_attempt(attempt);
                warn!(
                    attempt = attempt + 1,
                    max = policy.max_retries,
                    delay_ms = delay.as_millis() as u64,
                    error = %err,
                    "Retrying after transient error"
                );
                tokio::select! {
                    _ = token.cancelled() => {
                        return Err(HlsDownloaderError::Cancelled);
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }
    }

    // Unreachable: the loop covers 0..=max_retries and the last iteration returns on Retry.
    Err(HlsDownloaderError::Internal {
        reason: "retry loop exited without result".to_string(),
    })
}

/// Classify a reqwest error as retryable or non-retryable.
///
/// Retryable: connect, timeout, request, body read, and decode errors.
/// Non-retryable: redirect and builder errors.
pub fn is_retryable_reqwest_error(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn delay_respects_max_cap() {
        let policy = RetryPolicy {
            max_retries: 10,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(5),
            jitter: false,
        };
        // attempt 10: 500ms * 2^10 = 512_000ms, should be capped to 5s
        let delay = policy.delay_for_attempt(10);
        assert!(delay <= Duration::from_secs(5));
    }

    #[test]
    fn delay_with_jitter_does_not_exceed_max_cap() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(1),
            jitter: true,
        };

        // Run a few times to sample jitter outcomes.
        for _ in 0..32 {
            let delay = policy.delay_for_attempt(10);
            assert!(delay <= Duration::from_secs(1));
        }
    }

    #[test]
    fn delay_without_jitter_is_deterministic() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter: false,
        };
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(400));
    }

    #[test]
    fn delay_with_jitter_adds_random_component() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter: true,
        };
        let delay = policy.delay_for_attempt(0);
        // Base is 100ms, jitter range is [0, 50ms), so delay should be in [100, 150)ms
        assert!(delay >= Duration::from_millis(100));
        assert!(delay < Duration::from_millis(150));
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_attempt() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            jitter: false,
        };
        let token = CancellationToken::new();
        let result =
            retry_with_backoff(&policy, &token, |_| async { RetryAction::Success(42u32) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_fails_immediately_on_non_retryable() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            jitter: false,
        };
        let token = CancellationToken::new();
        let attempts = AtomicU32::new(0);
        let result: Result<u32, _> = retry_with_backoff(&policy, &token, |_| {
            attempts.fetch_add(1, Ordering::Relaxed);
            async {
                RetryAction::Fail(HlsDownloaderError::SegmentFetch {
                    reason: "404 not found".to_string(),
                    retryable: false,
                })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn retry_exhausts_then_fails() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_secs(1),
            jitter: false,
        };
        let token = CancellationToken::new();
        let attempts = AtomicU32::new(0);
        let result: Result<u32, _> = retry_with_backoff(&policy, &token, |_| {
            attempts.fetch_add(1, Ordering::Relaxed);
            async {
                RetryAction::Retry(HlsDownloaderError::SegmentFetch {
                    reason: "500 internal".to_string(),
                    retryable: true,
                })
            }
        })
        .await;
        assert!(result.is_err());
        // Initial attempt + 2 retries = 3 total
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn retry_succeeds_on_second_attempt() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_secs(1),
            jitter: false,
        };
        let token = CancellationToken::new();
        let attempts = AtomicU32::new(0);
        let result = retry_with_backoff(&policy, &token, |attempt| {
            attempts.fetch_add(1, Ordering::Relaxed);
            async move {
                if attempt == 0 {
                    RetryAction::Retry(HlsDownloaderError::SegmentFetch {
                        reason: "timeout".to_string(),
                        retryable: true,
                    })
                } else {
                    RetryAction::Success(99u32)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn retry_respects_cancellation() {
        let policy = RetryPolicy {
            max_retries: 10,
            base_delay: Duration::from_secs(100),
            max_delay: Duration::from_secs(100),
            jitter: false,
        };
        let token = CancellationToken::new();
        token.cancel();
        let result: Result<u32, _> =
            retry_with_backoff(&policy, &token, |_| async { RetryAction::Success(1u32) }).await;
        assert!(matches!(result, Err(HlsDownloaderError::Cancelled)));
    }
}
