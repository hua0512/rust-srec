//! Shared platform admission and one deadline for a logical credential operation.

use std::future::Future;
use std::time::Duration;

use platforms_parser::extractor::error::{ExtractorError, ThrottleScope};
use sqlx::SqlitePool;
use tokio::time::Instant;

use crate::monitor::{RateLimiterConfig, RateLimiterManager, StreamMonitorConfig};

use super::{CredentialError, CredentialScope, CredentialSource};

/// Applied when a platform throttles without saying for how long.
const DEFAULT_PLATFORM_BACKOFF: Duration = Duration::from_secs(60);

/// Refresh, lock acquisition, provider checks and extraction share this budget.
#[derive(Debug, Clone, Copy)]
pub struct OperationDeadline(Instant);

impl OperationDeadline {
    pub fn new(duration: Duration) -> Self {
        Self(Instant::now() + duration)
    }

    pub fn instant(self) -> Instant {
        self.0
    }

    pub async fn run<T>(
        self,
        future: impl Future<Output = Result<T, CredentialError>>,
    ) -> Result<T, CredentialError> {
        tokio::time::timeout_at(self.0, future)
            .await
            .map_err(|_| CredentialError::DeadlineExceeded)?
    }
}

impl Default for OperationDeadline {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

/// All accounts and API entrypoints for a stored platform share its bucket.
pub struct PlatformAdmission {
    limiter: RateLimiterManager,
    pool: Option<SqlitePool>,
    // None represents a remote delay beyond Instant's range. It remains blocked
    // until the operation deadline, rather than panicking or shortening Retry-After.
    backoff: dashmap::DashMap<String, Option<Instant>>,
}

impl PlatformAdmission {
    pub fn new(limiter: RateLimiterManager) -> Self {
        Self {
            limiter,
            pool: None,
            backoff: dashmap::DashMap::new(),
        }
    }

    pub fn from_config(config: &StreamMonitorConfig) -> Self {
        let default =
            RateLimiterConfig::with_rps(config.default_rate_limit).unwrap_or_else(|error| {
                tracing::warn!(%error, "Invalid platform admission rate; using default");
                RateLimiterConfig::default()
            });
        let mut limiter = RateLimiterManager::with_config(default);
        for (platform, rate) in &config.platform_rate_limits {
            match RateLimiterConfig::with_rps(*rate) {
                Ok(config) => limiter.set_platform_config(platform, config),
                Err(error) => {
                    tracing::warn!(%error, %platform, "Ignoring invalid platform admission rate")
                }
            }
        }
        Self::new(limiter)
    }

    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
    }

    pub(crate) fn limiter(&self) -> RateLimiterManager {
        self.limiter.clone()
    }

    pub fn register_platform_name(&mut self, platform_id: &str, platform_name: &str) {
        self.limiter
            .alias_platform_config(platform_id, &platform_name.to_ascii_lowercase());
    }

    pub async fn admit(
        &self,
        platform_id: &str,
        deadline: OperationDeadline,
    ) -> Result<(), CredentialError> {
        self.admit_with_token(platform_id, deadline, self.limiter.acquire(platform_id))
            .await
    }

    async fn wait_backoff(&self, platform_id: &str) {
        loop {
            let until = self.backoff.get(platform_id).map(|entry| *entry);
            match until {
                Some(Some(until)) if until > Instant::now() => {
                    tokio::time::sleep_until(until).await
                }
                Some(None) => std::future::pending::<()>().await,
                _ => break,
            }
        }
    }

    async fn admit_with_token(
        &self,
        platform_id: &str,
        deadline: OperationDeadline,
        token: impl Future<Output = Duration>,
    ) -> Result<(), CredentialError> {
        deadline
            .run(async {
                self.wait_backoff(platform_id).await;
                token.await;
                // Another operation can report platform throttling while this one
                // waits for a bucket token. That backoff still gates network use.
                self.wait_backoff(platform_id).await;
                Ok(())
            })
            .await
    }

    /// A platform or unclassified throttle describes the shared bucket rather
    /// than one account, so it delays every caller of the platform whether the
    /// request carried a profile, legacy or raw cookies, or none at all.
    pub fn observe(&self, platform_id: &str, error: &ExtractorError) {
        if let ExtractorError::RateLimited {
            scope: ThrottleScope::Platform | ThrottleScope::Unknown,
            retry_after,
            ..
        } = error
        {
            self.defer(platform_id, retry_after.unwrap_or(DEFAULT_PLATFORM_BACKOFF));
        }
    }

    pub fn defer(&self, platform_id: &str, duration: Duration) {
        let until = Instant::now().checked_add(duration);
        self.backoff
            .entry(platform_id.to_owned())
            .and_modify(|current| {
                *current = match (*current, until) {
                    (Some(current), Some(until)) => Some(current.max(until)),
                    _ => None,
                };
            })
            .or_insert(until);
    }

    /// Legacy sources identify a platform by name. Resolve that name to the
    /// same stored identity used by monitoring and managed profile execution.
    async fn platform_id_for_name(&self, name: &str) -> Result<String, CredentialError> {
        let Some(pool) = &self.pool else {
            return Ok(name.to_ascii_lowercase());
        };
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM platform_config WHERE platform_name = ? COLLATE NOCASE ORDER BY id",
        )
        .bind(name)
        .fetch_all(pool)
        .await?;
        match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] => Ok(name.to_ascii_lowercase()),
            _ => Err(CredentialError::Internal(
                "Ambiguous platform name; select a platform ID".into(),
            )),
        }
    }

    async fn source_platform_id(
        &self,
        source: &CredentialSource,
    ) -> Result<String, CredentialError> {
        match &source.scope {
            CredentialScope::Platform { platform_id, .. } => Ok(platform_id.clone()),
            _ => self.platform_id_for_name(&source.platform_name).await,
        }
    }

    pub async fn admit_platform_name(
        &self,
        name: &str,
        deadline: OperationDeadline,
    ) -> Result<(), CredentialError> {
        deadline
            .run(async {
                let platform_id = self.platform_id_for_name(name).await?;
                self.admit(&platform_id, deadline).await
            })
            .await
    }

    pub async fn admit_source(
        &self,
        source: &CredentialSource,
        deadline: OperationDeadline,
    ) -> Result<(), CredentialError> {
        deadline
            .run(async {
                let platform_id = self.source_platform_id(source).await?;
                self.admit(&platform_id, deadline).await
            })
            .await
    }

    pub fn observe_provider(&self, platform_id: &str, error: &CredentialError) {
        if let CredentialError::RateLimited { retry_after } = error {
            self.defer(platform_id, retry_after.unwrap_or(DEFAULT_PLATFORM_BACKOFF));
        }
    }

    pub async fn observe_platform_name(&self, name: &str, error: &CredentialError) {
        if !matches!(error, CredentialError::RateLimited { .. }) {
            return;
        }
        match self.platform_id_for_name(name).await {
            Ok(platform_id) => self.observe_provider(&platform_id, error),
            Err(error) => {
                tracing::warn!(%error, platform = name, "Could not apply provider throttle backoff")
            }
        }
    }

    /// Legacy sources and managed profiles defer the same platform bucket.
    pub async fn observe_source(&self, source: &CredentialSource, error: &CredentialError) {
        if !matches!(error, CredentialError::RateLimited { .. }) {
            return;
        }
        match self.source_platform_id(source).await {
            Ok(platform_id) => self.observe_provider(&platform_id, error),
            Err(error) => {
                tracing::warn!(%error, platform = %source.platform_name, "Could not apply platform backoff for a throttled credential provider")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_throttle_report_while_waiting_for_a_token_still_delays_admission() {
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        let release_token = tokio::sync::Notify::new();
        let token = async {
            release_token.notified().await;
            Duration::ZERO
        };
        let mut request = Box::pin(admission.admit_with_token(
            "platform",
            OperationDeadline::new(Duration::from_secs(120)),
            token,
        ));
        assert!(futures::poll!(request.as_mut()).is_pending());
        admission.defer("platform", Duration::from_secs(60));
        release_token.notify_one();
        assert!(
            futures::poll!(request.as_mut()).is_pending(),
            "a token must not bypass a newer platform backoff"
        );
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(futures::poll!(request.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(request.await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn only_shared_throttles_delay_the_platform() {
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        let throttled = |scope| ExtractorError::RateLimited {
            scope,
            code: None,
            retry_after: None,
        };
        admission.observe("account", &throttled(ThrottleScope::Account));
        admission.observe(
            "account",
            &ExtractorError::Authentication {
                code: "-101".into(),
            },
        );
        admission.observe("unknown", &throttled(ThrottleScope::Unknown));
        admission.observe(
            "platform",
            &ExtractorError::RateLimited {
                scope: ThrottleScope::Platform,
                code: None,
                retry_after: Some(Duration::from_secs(5)),
            },
        );
        let origin = Instant::now();
        let quick = OperationDeadline::new(Duration::from_secs(1));
        assert!(admission.admit("account", quick).await.is_ok());
        assert!(matches!(
            admission.admit("unknown", quick).await,
            Err(CredentialError::DeadlineExceeded)
        ));
        admission
            .admit("platform", OperationDeadline::new(Duration::from_secs(10)))
            .await
            .unwrap();
        assert_eq!(origin.elapsed(), Duration::from_secs(5));
        admission
            .admit("unknown", OperationDeadline::new(Duration::from_secs(120)))
            .await
            .unwrap();
        assert_eq!(origin.elapsed(), DEFAULT_PLATFORM_BACKOFF);
    }

    #[tokio::test(start_paused = true)]
    async fn unrepresentable_remote_delay_is_bounded_by_the_operation_deadline() {
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.defer("platform", Duration::MAX);
        admission.defer("platform", Duration::from_secs(1));
        let result = admission
            .admit("platform", OperationDeadline::new(Duration::from_secs(3)))
            .await;
        assert!(matches!(result, Err(CredentialError::DeadlineExceeded)));
        assert!(
            admission
                .admit("other", OperationDeadline::new(Duration::from_secs(3)))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn admission_wait_uses_remaining_budget_and_cancellation_consumes_no_token() {
        let limiter = RateLimiterManager::with_config(RateLimiterConfig {
            max_tokens: 1,
            refill_rate: 0.01,
            initial_tokens: 1,
        });
        let admission = PlatformAdmission::new(limiter.clone());
        admission
            .admit("platform", OperationDeadline::default())
            .await
            .unwrap();
        let started = Instant::now();
        let deadline = OperationDeadline::new(Duration::from_millis(20));
        assert!(matches!(
            admission.admit("platform", deadline).await,
            Err(CredentialError::DeadlineExceeded)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(limiter.available_tokens("platform").await < 0.1);
        // Independent platforms retain their own configured capacity.
        admission
            .admit("other", OperationDeadline::new(Duration::from_millis(20)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn provider_wait_cannot_restart_an_expired_operation_budget() {
        let deadline = OperationDeadline::new(Duration::ZERO);
        let outcome = deadline
            .run(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            })
            .await;
        assert!(matches!(outcome, Err(CredentialError::DeadlineExceeded)));
    }
}
