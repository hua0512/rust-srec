//! Shared platform admission and one deadline for a logical credential operation.

use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use platforms_parser::extractor::error::ExtractorError;
use tokio::time::Instant;

use crate::monitor::{RateLimiterConfig, RateLimiterManager, StreamMonitorConfig};
use crate::notification::{NotificationEvent, NotificationService};

use crate::proxies::{ResolvedRoute, RouteKey};

use super::CredentialError;

/// Applied when a platform throttles without saying for how long.
const DEFAULT_PLATFORM_BACKOFF: Duration = Duration::from_secs(60);

/// Upper bound on a delay requested by a remote server. A single 429 from a
/// CDN or WAF can carry an arbitrary `Retry-After`; honouring it verbatim would
/// stop every check on the platform (or keep an account excluded across
/// restarts) for as long as that server asked.
pub(crate) const MAX_REMOTE_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// Clamp a remote throttle delay to [`MAX_REMOTE_BACKOFF`].
pub(crate) fn bounded_remote_delay(delay: Duration) -> Duration {
    delay.min(MAX_REMOTE_BACKOFF)
}

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

/// All accounts and API entrypoints for a stored platform share its request
/// rate. A throttle pauses the network path it was seen on: direct, the
/// environment's proxy, or one saved proxy.
pub struct PlatformAdmission {
    limiter: RateLimiterManager,
    backoff: dashmap::DashMap<(String, RouteKey), Instant>,
    /// Display names for notifications; unregistered platforms show their ID.
    platform_names: dashmap::DashMap<String, String>,
    notification_service: OnceLock<Arc<NotificationService>>,
}

impl PlatformAdmission {
    pub fn new(limiter: RateLimiterManager) -> Self {
        Self {
            limiter,
            backoff: dashmap::DashMap::new(),
            platform_names: dashmap::DashMap::new(),
            notification_service: OnceLock::new(),
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

    pub(crate) fn limiter(&self) -> RateLimiterManager {
        self.limiter.clone()
    }

    pub fn register_platform_name(&mut self, platform_id: &str, platform_name: &str) {
        self.limiter
            .alias_platform_config(platform_id, &platform_name.to_ascii_lowercase());
        self.platform_names
            .insert(platform_id.to_owned(), platform_name.to_owned());
    }

    /// Backoffs are reported once when they start; extensions of an active
    /// backoff are not re-announced.
    pub fn set_notification_service(&self, service: Arc<NotificationService>) {
        let _ = self.notification_service.set(service);
    }

    pub async fn admit(
        &self,
        platform_id: &str,
        key: &RouteKey,
        deadline: OperationDeadline,
    ) -> Result<(), CredentialError> {
        self.admit_with_token(
            platform_id,
            key,
            deadline,
            self.limiter.acquire(platform_id),
        )
        .await
    }

    fn backoff_until(&self, platform_id: &str, key: &RouteKey) -> Option<Instant> {
        self.backoff
            .get(&(platform_id.to_owned(), key.clone()))
            .map(|entry| *entry)
            .filter(|until| *until > Instant::now())
    }

    /// Whether requests on this path are paused by a throttle.
    pub fn backing_off(&self, platform_id: &str, key: &RouteKey) -> bool {
        self.backoff_until(platform_id, key).is_some()
    }

    /// Forgets the throttles of a path on every platform, for a saved proxy
    /// that now reaches another exit or no longer exists.
    pub fn clear_route(&self, key: &RouteKey) {
        self.backoff.retain(|(_, stored), _| stored != key);
    }

    async fn wait_backoff(&self, platform_id: &str, key: &RouteKey) {
        while let Some(until) = self.backoff_until(platform_id, key) {
            tokio::time::sleep_until(until).await;
        }
    }

    async fn admit_with_token(
        &self,
        platform_id: &str,
        key: &RouteKey,
        deadline: OperationDeadline,
        token: impl Future<Output = Duration>,
    ) -> Result<(), CredentialError> {
        deadline
            .run(async {
                self.wait_backoff(platform_id, key).await;
                token.await;
                // Another operation can report throttling while this one
                // waits for a bucket token. That backoff still gates network use.
                self.wait_backoff(platform_id, key).await;
                Ok(())
            })
            .await
    }

    /// A throttle describes the network path rather than one account, so it
    /// delays every caller on that path whether the request carried a
    /// profile, raw cookies, or none at all.
    pub fn observe(&self, platform_id: &str, route: &ResolvedRoute, error: &ExtractorError) {
        if let ExtractorError::RateLimited { retry_after, .. } = error {
            self.defer(
                platform_id,
                route,
                retry_after.unwrap_or(DEFAULT_PLATFORM_BACKOFF),
            );
        }
    }

    pub fn defer(&self, platform_id: &str, route: &ResolvedRoute, duration: Duration) {
        let duration = bounded_remote_delay(duration);
        let now = Instant::now();
        let until = now + duration;
        let started = match self
            .backoff
            .entry((platform_id.to_owned(), route.key.clone()))
        {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                let started = *entry.get() <= now;
                if until > *entry.get() {
                    entry.insert(until);
                }
                started
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(until);
                true
            }
        };
        if !started {
            return;
        }
        tracing::warn!(
            platform_id,
            route = route.kind().as_str(),
            proxy = route.proxy_name(),
            backoff_secs = duration.as_secs(),
            "Platform throttled; pausing its requests on this route"
        );
        if let Some(service) = self.notification_service.get() {
            let platform = self
                .platform_names
                .get(platform_id)
                .map_or_else(|| platform_id.to_owned(), |name| name.clone());
            let now = chrono::Utc::now();
            service.dispatch_notification(NotificationEvent::PlatformThrottled {
                platform,
                route: route.kind().as_str().to_owned(),
                proxy_name: route.proxy_name().map(str::to_owned),
                retry_at: now + duration,
                timestamp: now,
            });
        }
    }

    pub fn observe_provider(
        &self,
        platform_id: &str,
        route: &ResolvedRoute,
        error: &CredentialError,
    ) {
        if let CredentialError::RateLimited { retry_after } = error {
            self.defer(
                platform_id,
                route,
                retry_after.unwrap_or(DEFAULT_PLATFORM_BACKOFF),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxies::{ProxyEndpoint, ProxyName, ProxyTarget, RouteSource};

    fn direct() -> ResolvedRoute {
        ResolvedRoute::direct(RouteSource::Global)
    }

    fn proxy(id: &str) -> ResolvedRoute {
        ResolvedRoute {
            target: ProxyTarget::Explicit(ProxyEndpoint::new("http://proxy.example:8080", None)),
            proxy: Some(ProxyName {
                id: id.into(),
                name: format!("{id} name"),
            }),
            source: RouteSource::Account,
            key: RouteKey::Proxy { id: id.into() },
        }
    }

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
            &RouteKey::Direct,
            OperationDeadline::new(Duration::from_secs(120)),
            token,
        ));
        assert!(futures::poll!(request.as_mut()).is_pending());
        admission.defer("platform", &direct(), Duration::from_secs(60));
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
    async fn throttles_delay_the_platform_but_rejected_accounts_do_not() {
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.observe(
            "account",
            &direct(),
            &ExtractorError::Authentication {
                code: "-101".into(),
            },
        );
        admission.observe(
            "unstated",
            &direct(),
            &ExtractorError::RateLimited {
                code: None,
                retry_after: None,
            },
        );
        admission.observe(
            "stated",
            &direct(),
            &ExtractorError::RateLimited {
                code: None,
                retry_after: Some(Duration::from_secs(5)),
            },
        );
        let origin = Instant::now();
        let quick = OperationDeadline::new(Duration::from_secs(1));
        assert!(
            admission
                .admit("account", &RouteKey::Direct, quick)
                .await
                .is_ok()
        );
        assert!(matches!(
            admission.admit("unstated", &RouteKey::Direct, quick).await,
            Err(CredentialError::DeadlineExceeded)
        ));
        admission
            .admit(
                "stated",
                &RouteKey::Direct,
                OperationDeadline::new(Duration::from_secs(10)),
            )
            .await
            .unwrap();
        assert_eq!(origin.elapsed(), Duration::from_secs(5));
        admission
            .admit(
                "unstated",
                &RouteKey::Direct,
                OperationDeadline::new(Duration::from_secs(120)),
            )
            .await
            .unwrap();
        assert_eq!(origin.elapsed(), DEFAULT_PLATFORM_BACKOFF);
    }

    #[tokio::test(start_paused = true)]
    async fn a_throttle_pauses_only_the_route_it_was_seen_on_and_names_it() {
        let mut admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.register_platform_name("platform", "bilibili");
        let notifications = Arc::new(NotificationService::new());
        let mut received = notifications.subscribe();
        admission.set_notification_service(notifications);
        let throttled = |seconds| ExtractorError::RateLimited {
            code: None,
            retry_after: Some(Duration::from_secs(seconds)),
        };
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap()
        };
        admission.observe("platform", &proxy("one"), &throttled(300));
        let quick = OperationDeadline::new(Duration::from_secs(1));
        assert!(admission.backing_off("platform", &proxy("one").key));
        assert!(matches!(
            admission.admit("platform", &proxy("one").key, quick).await,
            Err(CredentialError::DeadlineExceeded)
        ));
        for other in [RouteKey::Direct, RouteKey::System, proxy("two").key] {
            assert!(!admission.backing_off("platform", &other));
            assert!(admission.admit("platform", &other, quick).await.is_ok());
        }
        // Every route's pause is announced, naming the route.
        let NotificationEvent::PlatformThrottled {
            platform,
            route,
            proxy_name,
            retry_at,
            timestamp,
        } = next().await
        else {
            panic!("expected a platform pause");
        };
        assert_eq!(
            (platform.as_str(), route.as_str(), proxy_name.as_deref()),
            ("bilibili", "proxy", Some("one name"))
        );
        assert_eq!((retry_at - timestamp).num_seconds(), 300);
        admission.observe("platform", &direct(), &throttled(30));
        let NotificationEvent::PlatformThrottled {
            route, proxy_name, ..
        } = next().await
        else {
            panic!("expected a platform pause");
        };
        assert_eq!((route.as_str(), proxy_name), ("direct", None));

        // A saved proxy that moved to another exit starts unthrottled.
        admission.clear_route(&proxy("one").key);
        assert!(!admission.backing_off("platform", &proxy("one").key));
        assert!(admission.backing_off("platform", &RouteKey::Direct));
    }

    #[tokio::test(start_paused = true)]
    async fn remote_delays_are_capped_so_one_429_cannot_stall_a_platform() {
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.defer("platform", &direct(), Duration::MAX);
        admission.defer("platform", &direct(), Duration::from_secs(1));
        let origin = Instant::now();
        let result = admission
            .admit(
                "platform",
                &RouteKey::Direct,
                OperationDeadline::new(Duration::from_secs(3)),
            )
            .await;
        assert!(matches!(result, Err(CredentialError::DeadlineExceeded)));
        assert!(
            admission
                .admit(
                    "other",
                    &RouteKey::Direct,
                    OperationDeadline::new(Duration::from_secs(3))
                )
                .await
                .is_ok()
        );
        admission
            .admit(
                "platform",
                &RouteKey::Direct,
                OperationDeadline::new(MAX_REMOTE_BACKOFF * 2),
            )
            .await
            .unwrap();
        assert_eq!(origin.elapsed(), MAX_REMOTE_BACKOFF);
    }

    #[tokio::test(start_paused = true)]
    async fn a_platform_pause_is_announced_when_it_starts_but_not_when_extended() {
        let mut admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.register_platform_name("platform-bilibili", "bilibili");
        let notifications = Arc::new(NotificationService::new());
        let mut received = notifications.subscribe();
        admission.set_notification_service(notifications);
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap()
        };

        admission.defer("platform-bilibili", &direct(), Duration::from_secs(30));
        admission.defer("platform-bilibili", &direct(), Duration::from_secs(60));
        let first = next().await;
        assert!(matches!(
            first,
            NotificationEvent::PlatformThrottled { ref platform, .. } if platform == "bilibili"
        ));

        tokio::time::advance(Duration::from_secs(61)).await;
        admission.defer("platform-bilibili", &direct(), Duration::from_secs(1));
        let second = next().await;
        // An announced extension would have resumed 60s out, after the first pause.
        let resume = |event: &NotificationEvent| match event {
            NotificationEvent::PlatformThrottled { retry_at, .. } => *retry_at,
            other => panic!("unexpected event {other:?}"),
        };
        assert!(resume(&second) < resume(&first));
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
            .admit("platform", &RouteKey::Direct, OperationDeadline::default())
            .await
            .unwrap();
        let started = Instant::now();
        let deadline = OperationDeadline::new(Duration::from_millis(20));
        assert!(matches!(
            admission
                .admit("platform", &RouteKey::Direct, deadline)
                .await,
            Err(CredentialError::DeadlineExceeded)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(limiter.available_tokens("platform").await < 0.1);
        // Independent platforms retain their own configured capacity.
        admission
            .admit(
                "other",
                &RouteKey::Direct,
                OperationDeadline::new(Duration::from_millis(20)),
            )
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
