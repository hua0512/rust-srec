//! Batch detection for platforms that support it.
//!
//! Some platforms allow checking the status of multiple streamers
//! in a single API call, which is more efficient than individual checks.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{debug, warn};

use crate::Result;
use crate::streamer::StreamerMetadata;

use super::detector::LiveStatus;
use super::rate_limiter::RateLimiterManager;

/// Result of a batch detection operation.
#[derive(Debug)]
pub struct BatchResult {
    /// Results by streamer ID.
    pub results: HashMap<String, LiveStatus>,
    /// Streamers that failed to check.
    pub failures: Vec<BatchFailure>,
}

/// A failure during batch detection.
#[derive(Debug)]
pub struct BatchFailure {
    /// Streamer ID.
    pub streamer_id: String,
    /// Error message.
    pub error: String,
}

impl BatchResult {
    /// Create a new empty batch result.
    pub fn new() -> Self {
        Self {
            results: HashMap::new(),
            failures: Vec::new(),
        }
    }

    /// Add a successful result.
    pub fn add_result(&mut self, streamer_id: String, status: LiveStatus) {
        self.results.insert(streamer_id, status);
    }

    /// Add a failure.
    pub fn add_failure(&mut self, streamer_id: String, error: String) {
        self.failures.push(BatchFailure { streamer_id, error });
    }

    /// Get the total number of streamers processed.
    pub fn total_count(&self) -> usize {
        self.results.len() + self.failures.len()
    }

    /// Get the number of successful results.
    pub fn success_count(&self) -> usize {
        self.results.len()
    }

    /// Get the number of failures.
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// Check if all streamers were processed successfully.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

impl Default for BatchResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Batch detector for checking multiple streamers at once.
pub struct BatchDetector {
    /// HTTP client for API requests.
    #[allow(dead_code)]
    client: reqwest::Client,
    /// Rate limiter manager.
    rate_limiter: RateLimiterManager,
    /// Maximum streamers per batch request.
    max_batch_size: usize,
    /// Retry delay on rate limit.
    retry_delay: Duration,
    /// Maximum retries on rate limit.
    max_retries: u32,
}

impl BatchDetector {
    /// Create a new batch detector.
    pub fn new(rate_limiter: RateLimiterManager) -> Self {
        Self {
            client: reqwest::Client::new(),
            rate_limiter,
            max_batch_size: 100,
            retry_delay: Duration::from_secs(5),
            max_retries: 3,
        }
    }

    /// Create a new batch detector with a custom HTTP client.
    pub fn with_client(client: reqwest::Client, rate_limiter: RateLimiterManager) -> Self {
        Self {
            client,
            rate_limiter,
            max_batch_size: 100,
            retry_delay: Duration::from_secs(5),
            max_retries: 3,
        }
    }

    /// Set the maximum batch size.
    pub fn with_max_batch_size(mut self, size: usize) -> Self {
        self.max_batch_size = size;
        self
    }

    /// Set the retry delay.
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Check the status of multiple streamers on the same platform.
    pub async fn batch_check(
        &self,
        platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<BatchResult> {
        debug!(
            "Batch checking {} streamers on platform {}",
            streamers.len(),
            platform_id
        );

        let mut result = BatchResult::new();

        // Split into batches if needed
        for chunk in streamers.chunks(self.max_batch_size) {
            // Acquire rate limit token
            let wait_time = self.rate_limiter.acquire(platform_id).await;
            if !wait_time.is_zero() {
                debug!("Rate limited for {:?}", wait_time);
            }

            // Perform batch check with retries
            match self.check_batch_with_retry(platform_id, chunk).await {
                Ok(batch_result) => {
                    for (id, status) in batch_result.results {
                        result.add_result(id, status);
                    }
                    for failure in batch_result.failures {
                        result.add_failure(failure.streamer_id, failure.error);
                    }
                }
                Err(e) => {
                    // Add all streamers in this batch as failures
                    for streamer in chunk {
                        result.add_failure(streamer.id.clone(), e.to_string());
                    }
                }
            }
        }

        debug!(
            "Batch check complete: {} success, {} failures",
            result.success_count(),
            result.failure_count()
        );

        Ok(result)
    }

    /// Check a batch with retry logic.
    async fn check_batch_with_retry(
        &self,
        platform_id: &str,
        streamers: &[StreamerMetadata],
    ) -> Result<BatchResult> {
        let mut retries = 0;

        loop {
            match self.check_batch_internal(platform_id, streamers).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if retries >= self.max_retries {
                        return Err(e);
                    }

                    // Check if it's a rate limit error
                    if is_rate_limit_error(&e) {
                        let delay = self.calculate_backoff(retries);
                        warn!(
                            "Rate limited on {}, retrying in {:?} (attempt {}/{})",
                            platform_id,
                            delay,
                            retries + 1,
                            self.max_retries
                        );
                        tokio::time::sleep(delay).await;
                        retries += 1;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Internal batch check implementation.
    ///
    /// This is a placeholder that will be replaced with actual platform integration.
    async fn check_batch_internal(
        &self,
        _platform_id: &str,
        streamers: &[StreamerMetadata],
    ) -> Result<BatchResult> {
        // Placeholder implementation
        // In the real implementation, this would:
        // 1. Build the batch API request for the platform
        // 2. Send the request
        // 3. Parse the response and map to LiveStatus

        let mut result = BatchResult::new();

        for streamer in streamers {
            // For now, return offline for all
            result.add_result(streamer.id.clone(), LiveStatus::Offline);
        }

        Ok(result)
    }

    /// Calculate backoff delay with exponential increase and jitter.
    fn calculate_backoff(&self, retry_count: u32) -> Duration {
        let base_delay = self.retry_delay.as_millis() as u64;
        let exponential_delay = base_delay.saturating_mul(2u64.saturating_pow(retry_count));

        // Add jitter (±25%)
        let jitter_range = exponential_delay / 4;
        let jitter: i64 = if jitter_range > 0 {
            let random_val = rand::random::<u64>() % (jitter_range.saturating_mul(2).max(1));
            random_val as i64 - jitter_range as i64
        } else {
            0
        };

        Duration::from_millis(exponential_delay.saturating_add_signed(jitter))
    }
}

/// Check if an error is a rate limit error.
fn is_rate_limit_error(error: &crate::Error) -> bool {
    // Check for HTTP 429 status or rate limit messages
    let error_str = error.to_string().to_lowercase();
    error_str.contains("429") || error_str.contains("rate limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Priority, StreamerState};

    fn create_test_streamer(id: &str) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: format!("Streamer {}", id),
            url: format!("https://twitch.tv/{}", id),
            platform_config_id: "twitch".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_error: None,
            last_live_time: None,
            streamer_specific_config: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_batch_result() {
        let mut result = BatchResult::new();

        result.add_result("streamer-1".to_string(), LiveStatus::Offline);
        result.add_result(
            "streamer-2".to_string(),
            LiveStatus::Live {
                title: "Test".to_string(),
                category: None,
                avatar: None,
                started_at: None,
                viewer_count: None,
                streams: vec![],
                media_headers: None,
                media_extras: None,
            },
        );
        result.add_failure("streamer-3".to_string(), "Network error".to_string());

        assert_eq!(result.total_count(), 3);
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 1);
        assert!(!result.is_complete());
    }

    #[tokio::test]
    async fn test_batch_detector() {
        let rate_limiter = RateLimiterManager::new();
        let detector = BatchDetector::new(rate_limiter);

        let streamers = vec![
            create_test_streamer("1"),
            create_test_streamer("2"),
            create_test_streamer("3"),
        ];

        let result = detector.batch_check("twitch", streamers).await.unwrap();

        assert_eq!(result.success_count(), 3);
        assert_eq!(result.failure_count(), 0);
    }

    #[test]
    fn test_calculate_backoff() {
        let rate_limiter = RateLimiterManager::new();
        let detector = BatchDetector::new(rate_limiter).with_retry_delay(Duration::from_secs(1));

        // First retry should be around 1 second
        let delay0 = detector.calculate_backoff(0);
        assert!(delay0 >= Duration::from_millis(750));
        assert!(delay0 <= Duration::from_millis(1250));

        // Second retry should be around 2 seconds
        let delay1 = detector.calculate_backoff(1);
        assert!(delay1 >= Duration::from_millis(1500));
        assert!(delay1 <= Duration::from_millis(2500));
    }
}
