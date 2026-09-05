//! Credential refresh service.
//!
//! Orchestrates credential checking, refreshing, and persistence.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument, warn};

use crate::database::repositories::config::ConfigRepository;
use crate::domain::streamer::Streamer;
use crate::notification::{NotificationEvent, NotificationService};
use crate::streamer::StreamerMetadata;

use super::error::CredentialError;
use super::manager::{CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials};
use super::resolver::CredentialResolver;
use super::store::CredentialStore;
use super::tracker::{DailyCheckTracker, RefreshFailureTracker};
use super::types::{CredentialEvent, CredentialScope, CredentialSource};

/// Credential refresh service.
///
/// Orchestrates detection, refresh, and persistence of platform credentials.
pub struct CredentialRefreshService<R: ConfigRepository> {
    resolver: Arc<CredentialResolver<R>>,
    store: Arc<dyn CredentialStore>,
    managers: HashMap<String, Arc<dyn CredentialManager>>,
    daily_tracker: Arc<DailyCheckTracker>,
    failure_tracker: Arc<RefreshFailureTracker>,
    /// Per-scope locks to prevent concurrent refreshes
    refresh_locks: dashmap::DashMap<String, Arc<Mutex<()>>>,
    /// Optional notification service for broadcasting credential events.
    notification_service: OnceLock<Arc<NotificationService>>,
}

impl<R: ConfigRepository + 'static> CredentialRefreshService<R> {
    /// Create a new credential refresh service.
    pub fn new(resolver: Arc<CredentialResolver<R>>, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            resolver,
            store,
            managers: HashMap::new(),
            daily_tracker: Arc::new(DailyCheckTracker::new()),
            failure_tracker: Arc::new(RefreshFailureTracker::new()),
            refresh_locks: dashmap::DashMap::new(),
            notification_service: OnceLock::new(),
        }
    }

    /// Wire a NotificationService to emit CredentialEvents as NotificationEvents.
    pub fn set_notification_service(&self, service: Arc<NotificationService>) {
        if self.notification_service.set(service).is_err() {
            warn!("Credential notification service is already configured");
        }
    }

    #[cfg(test)]
    pub(crate) fn has_notification_service(&self) -> bool {
        self.notification_service.get().is_some()
    }

    /// Register a credential manager for a platform.
    pub fn register_manager(&mut self, manager: Arc<dyn CredentialManager>) {
        let platform_id = manager.platform_id().to_string();
        self.managers.insert(platform_id, manager);
    }

    /// Get the daily check tracker (for testing or external access).
    pub fn daily_tracker(&self) -> Arc<DailyCheckTracker> {
        Arc::clone(&self.daily_tracker)
    }

    /// Get the failure tracker (for testing or external access).
    pub fn failure_tracker(&self) -> Arc<RefreshFailureTracker> {
        Arc::clone(&self.failure_tracker)
    }

    /// Check and refresh credentials for a streamer if needed.
    ///
    /// Uses the once-per-day check strategy: only calls the platform API
    /// once per day per credential scope.
    ///
    /// # Returns
    /// * `Ok(Some(new_cookies))` - Credentials were refreshed
    /// * `Ok(None)` - Credentials are valid, no refresh needed
    /// * `Err(...)` - Error during check or refresh
    #[instrument(skip_all, fields(streamer_id = %streamer.id, streamer_name = %streamer.name))]
    pub async fn check_and_refresh(
        &self,
        streamer: &Streamer,
    ) -> Result<Option<String>, CredentialError> {
        // Find credential source
        let source = match self.resolver.find_cookie_source(streamer).await? {
            Some(s) => s,
            None => {
                debug!("No credentials configured");
                return Ok(None);
            }
        };

        self.check_and_refresh_source(&source).await
    }

    /// Check and refresh credentials for a pre-resolved credential source.
    ///
    /// This is useful for hot paths that already loaded platform/template records (e.g. config
    /// resolution) and want to avoid extra DB queries just to find credential provenance.
    #[instrument(skip_all, fields(platform = %source.platform_name, scope = %source.scope.describe()))]
    pub async fn check_and_refresh_source(
        &self,
        source: &CredentialSource,
    ) -> Result<Option<String>, CredentialError> {
        // Skip platforms without a registered credential manager (unsupported for auto-refresh).
        let platform_key = source.platform_name.to_ascii_lowercase();
        if !self.managers.contains_key(&platform_key)
            && !self.managers.contains_key(&source.platform_name)
        {
            // debug!(
            //     platform = %source.platform_name,
            //     "Platform does not support credential auto-refresh; skipping"
            // );
            return Ok(None);
        }

        // Cached NeedsRefresh can call the provider, so it shares the same
        // ownership as an initial check and the resulting persistence.
        let lock = self.get_refresh_lock(&source.scope);
        let _guard = lock.lock().await;

        let current = self.store.reload_source(source).await?;
        let refreshed = if let Some(status) = self.daily_tracker.get_cached_status(&source.scope) {
            self.handle_cached_status(&current, status).await?
        } else {
            self.perform_check_and_refresh(&current).await?
        };
        // Waiters may still hold an extractor configuration assembled before
        // the owner rotated its cookies. Return the committed cookies to them
        // as well, without calling the provider or notifying a second time.
        Ok(refreshed.or_else(|| (current.cookies != source.cookies).then_some(current.cookies)))
    }

    /// Check and refresh credentials for a StreamerMetadata.
    ///
    /// This is the method used by StreamMonitor integration.
    /// Uses the once-per-day check strategy.
    ///
    /// # Returns
    /// * `Ok(Some(new_cookies))` - Credentials were refreshed
    /// * `Ok(None)` - Credentials are valid, no refresh needed
    /// * `Err(...)` - Error during check or refresh
    #[instrument(skip_all, fields(streamer_id = %metadata.id, streamer_name = %metadata.name))]
    pub async fn check_and_refresh_for_metadata(
        &self,
        metadata: &StreamerMetadata,
    ) -> Result<Option<String>, CredentialError> {
        // Find credential source
        let source = match self
            .resolver
            .find_cookie_source_for_metadata(metadata)
            .await?
        {
            Some(s) => s,
            None => {
                debug!("No credentials configured");
                return Ok(None);
            }
        };

        self.check_and_refresh_source(&source).await
    }

    /// Handle a cached status from earlier today.
    async fn handle_cached_status(
        &self,
        source: &CredentialSource,
        status: CredentialStatus,
    ) -> Result<Option<String>, CredentialError> {
        match status {
            CredentialStatus::Valid => {
                debug!("Using cached valid status from today");
                Ok(None)
            }
            CredentialStatus::NeedsRefresh { .. } => {
                debug!("Cached status indicates refresh needed");
                // Attempt refresh
                self.perform_refresh(source).await
            }
            CredentialStatus::Invalid { reason, .. } => {
                debug!("Cached status indicates invalid credentials");
                Err(CredentialError::InvalidCredentials(reason))
            }
        }
    }

    /// Perform the actual check and refresh.
    async fn perform_check_and_refresh(
        &self,
        source: &CredentialSource,
    ) -> Result<Option<String>, CredentialError> {
        let manager = self.get_manager(&source.platform_name)?;

        info!(
            platform = %source.platform_name,
            scope = %source.scope.describe(),
            "Checking credential status"
        );

        let status = match manager.check_status(&source.cookies).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "Status check failed");
                // Don't cache failures - allow retry
                return Err(e);
            }
        };

        // Record the result for today
        self.daily_tracker
            .record_check(&source.scope, status.clone());

        // Also persist to DB for hydration on restart
        let result_str = match &status {
            CredentialStatus::Valid => "valid",
            CredentialStatus::NeedsRefresh { .. } => "needs_refresh",
            CredentialStatus::Invalid { .. } => "invalid",
        };
        if let Err(e) = self
            .store
            .update_check_result(&source.scope, result_str)
            .await
        {
            warn!(error = %e, "Failed to persist check result (non-fatal)");
        }

        match status {
            CredentialStatus::Valid => {
                info!("Credentials are valid");
                // Clear any previous failures
                self.failure_tracker.clear(&source.scope);
                Ok(None)
            }
            CredentialStatus::NeedsRefresh { refresh_deadline } => {
                info!(?refresh_deadline, "Credentials need refresh");
                self.perform_refresh(source).await
            }
            CredentialStatus::Invalid { reason, error_code } => {
                error!(%reason, ?error_code, "Credentials are invalid - manual re-login required");

                // Emit a notification event once per day (this path runs only on uncached checks).
                self.maybe_notify_credential_event(CredentialEvent::Invalid {
                    scope: source.scope.clone(),
                    platform: source.platform_name.clone(),
                    reason: reason.clone(),
                    error_code,
                    timestamp: Utc::now(),
                });

                Err(CredentialError::InvalidCredentials(reason))
            }
        }
    }

    fn maybe_notify_credential_event(&self, event: CredentialEvent) {
        let Some(service) = self.notification_service.get().cloned() else {
            return;
        };

        // Basic anti-spam gating for recurring failures.
        if let CredentialEvent::RefreshFailed {
            requires_relogin,
            failure_count,
            ..
        } = &event
        {
            let should_notify = *requires_relogin || *failure_count == 1 || *failure_count % 3 == 0;
            if !should_notify {
                return;
            }
        }

        service.dispatch_notification(NotificationEvent::Credential { event });
    }

    /// Perform credential refresh.
    #[instrument(skip_all, fields(platform = %source.platform_name, scope = %source.scope.describe()))]
    async fn perform_refresh(
        &self,
        source: &CredentialSource,
    ) -> Result<Option<String>, CredentialError> {
        let manager = self.get_manager(&source.platform_name)?;

        // Refresh token is required for OAuth-style platforms. Password re-login
        // platforms (SOOP) use reauth_extra instead.
        if !source.has_refresh_token() && !source.has_reauth_extra() {
            warn!("Missing refresh_token / reauth credentials - cannot auto-refresh");
            let _failure_count = self
                .failure_tracker
                .record_failure(&source.scope, "Missing refresh token");
            return Err(CredentialError::MissingRefreshToken);
        }

        info!("Starting credential refresh");

        let mut state = RefreshState::new(source.cookies.clone(), source.refresh_token.clone());
        // Pass access_token and/or password reauth material through extra JSON.
        let mut extra = serde_json::Map::new();
        if let Some(ref access_token) = source.access_token {
            extra.insert(
                "access_token".to_string(),
                serde_json::Value::String(access_token.clone()),
            );
        }
        if let Some(serde_json::Value::Object(map)) = source.reauth_extra.clone() {
            for (k, v) in map {
                extra.insert(k, v);
            }
        }
        if !extra.is_empty() {
            state.extra = Some(serde_json::Value::Object(extra));
        }

        match manager.refresh(&state).await {
            Ok(new_creds) => {
                info!(
                    expires_at = ?new_creds.expires_at,
                    "Credential refresh successful"
                );

                // Persist to database
                self.store.update_credentials(source, &new_creds).await?;

                // Update daily tracker with valid status
                self.daily_tracker
                    .record_check(&source.scope, CredentialStatus::Valid);

                // Clear failure tracking
                self.failure_tracker.clear(&source.scope);

                self.maybe_notify_credential_event(
                    self.create_refresh_success_event(source, &new_creds),
                );

                Ok(Some(new_creds.cookies))
            }
            Err(e) => {
                if e.requires_relogin() {
                    let reason = match &e {
                        CredentialError::InvalidCredentials(r) => r.clone(),
                        _ => e.to_string(),
                    };

                    // Cache an invalid status so we don't repeatedly attempt refresh within the day
                    // when the platform indicates a manual re-login is required.
                    self.daily_tracker.record_check(
                        &source.scope,
                        CredentialStatus::Invalid {
                            reason: reason.clone(),
                            error_code: None,
                        },
                    );

                    // Best-effort: persist invalid status for hydration on restart.
                    if let Err(store_err) = self
                        .store
                        .update_check_result(&source.scope, "invalid")
                        .await
                    {
                        warn!(error = %store_err, "Failed to persist invalid check result (non-fatal)");
                    }
                }

                let failure_count = self
                    .failure_tracker
                    .record_failure(&source.scope, &e.to_string());

                error!(
                    error = %e,
                    %failure_count,
                    "Credential refresh failed"
                );

                self.maybe_notify_credential_event(self.create_refresh_failed_event(source, &e));

                Err(e)
            }
        }
    }

    /// Get a credential manager for a platform.
    fn get_manager(
        &self,
        platform_name: &str,
    ) -> Result<&Arc<dyn CredentialManager>, CredentialError> {
        if let Some(manager) = self.managers.get(platform_name) {
            return Ok(manager);
        }
        // Platform ids are stored lowercase (e.g. "soop"); display names may differ.
        let key = platform_name.to_ascii_lowercase();
        self.managers
            .get(&key)
            .ok_or_else(|| CredentialError::UnsupportedPlatform(platform_name.to_string()))
    }

    /// Get or create a refresh lock for a scope.
    fn get_refresh_lock(&self, scope: &CredentialScope) -> Arc<Mutex<()>> {
        let key = scope.cache_key();
        self.refresh_locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Invalidate cached status for a scope (e.g., after user updates cookies).
    pub fn invalidate(&self, scope: &CredentialScope) {
        self.daily_tracker.invalidate(scope);
        self.failure_tracker.clear(scope);
    }

    /// Persist session cookies minted during extract (e.g. SOOP reactive login).
    ///
    /// Updates the same configuration layer that supplied the credential source
    /// and marks today's check status as valid.
    pub async fn persist_session_cookies(
        &self,
        source: &CredentialSource,
        cookies: String,
    ) -> Result<(), CredentialError> {
        if cookies.trim().is_empty() {
            return Ok(());
        }

        let lock = self.get_refresh_lock(&source.scope);
        let _guard = lock.lock().await;
        let current = self.store.reload_source(source).await?;
        let new_creds = RefreshedCredentials {
            cookies,
            refresh_token: current.refresh_token.clone(),
            access_token: current.access_token.clone(),
            expires_at: None,
        };

        self.store.update_credentials(source, &new_creds).await?;
        self.daily_tracker
            .record_check(&source.scope, CredentialStatus::Valid);
        self.failure_tracker.clear(&source.scope);
        if let Err(e) = self.store.update_check_result(&source.scope, "valid").await {
            warn!(
                platform = %source.platform_name,
                scope = %source.scope.describe(),
                error = %e,
                "Failed to persist credential check status"
            );
        }
        info!(
            platform = %source.platform_name,
            scope = %source.scope.describe(),
            "Persisted session cookies from extract"
        );
        Ok(())
    }

    /// Create a credential event for notification.
    pub fn create_refresh_failed_event(
        &self,
        source: &CredentialSource,
        error: &CredentialError,
    ) -> CredentialEvent {
        let failure_count = self.failure_tracker.failure_count(&source.scope);

        CredentialEvent::RefreshFailed {
            scope: source.scope.clone(),
            platform: source.platform_name.clone(),
            error: error.to_string(),
            requires_relogin: error.requires_relogin(),
            failure_count,
            timestamp: Utc::now(),
        }
    }

    /// Create a credential event for successful refresh.
    pub fn create_refresh_success_event(
        &self,
        source: &CredentialSource,
        credentials: &RefreshedCredentials,
    ) -> CredentialEvent {
        CredentialEvent::Refreshed {
            scope: source.scope.clone(),
            platform: source.platform_name.clone(),
            expires_at: credentials.expires_at,
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use sqlx::sqlite::SqlitePoolOptions;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::credentials::types::CredentialScope;
    use crate::database::repositories::{SqlxCredentialStore, config::SqlxConfigRepository};

    struct PausedRefresh {
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }

    #[async_trait::async_trait]
    impl CredentialManager for PausedRefresh {
        fn platform_id(&self) -> &'static str {
            "bilibili"
        }

        async fn check_status(&self, _cookies: &str) -> Result<CredentialStatus, CredentialError> {
            Ok(CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            })
        }

        async fn refresh(
            &self,
            state: &RefreshState,
        ) -> Result<RefreshedCredentials, CredentialError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(state.cookies, "stored-cookie");
            assert_eq!(state.refresh_token.as_deref(), Some("stored-token"));
            self.started.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok(RefreshedCredentials {
                cookies: "rotated-cookie".to_string(),
                refresh_token: Some("rotated-token".to_string()),
                access_token: None,
                expires_at: None,
            })
        }

        async fn validate(&self, _cookies: &str) -> Result<bool, CredentialError> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn cached_refresh_waits_for_its_owner_and_reads_current_credentials() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        sqlx::query("UPDATE platform_config SET cookies = 'stored-cookie', platform_specific_config = '{\"refresh_token\":\"stored-token\"}' WHERE id = 'platform-bilibili'")
            .execute(&pool).await.unwrap();
        let resolver = Arc::new(CredentialResolver::new(Arc::new(
            SqlxConfigRepository::new(pool.clone(), pool.clone()),
        )));
        let store = Arc::new(SqlxCredentialStore::new(pool.clone(), pool.clone()));
        let provider = Arc::new(PausedRefresh {
            calls: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        let mut service = CredentialRefreshService::new(resolver, store);
        service.register_manager(provider.clone());
        let service = Arc::new(service);
        let source = CredentialSource::new(
            CredentialScope::Platform {
                platform_id: "platform-bilibili".to_string(),
                platform_name: "bilibili".to_string(),
            },
            "stale-cookie".to_string(),
            Some("stale-token".to_string()),
            "bilibili".to_string(),
        );
        service.daily_tracker.record_check(
            &source.scope,
            CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            },
        );
        let first = {
            let service = service.clone();
            let source = source.clone();
            tokio::spawn(async move { service.check_and_refresh_source(&source).await })
        };
        tokio::time::timeout(Duration::from_secs(2), provider.started.notified())
            .await
            .unwrap();
        let second = service.check_and_refresh_source(&source);
        tokio::pin!(second);
        assert!(futures::poll!(second.as_mut()).is_pending());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        provider.release.add_permits(1);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), first)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .as_deref(),
            Some("rotated-cookie")
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), second)
                .await
                .unwrap()
                .unwrap()
                .as_deref(),
            Some("rotated-cookie"),
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let token: String = sqlx::query_scalar("SELECT json_extract(platform_specific_config, '$.refresh_token') FROM platform_config WHERE id = 'platform-bilibili'").fetch_one(&pool).await.unwrap();
        assert_eq!(token, "rotated-token");
    }

    /// `MakeWriter` that appends every formatted record to a buffer the test can read.
    #[derive(Clone, Default)]
    struct CapturedLog(Arc<Mutex<Vec<u8>>>);

    impl CapturedLog {
        fn contents(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner());
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl io::Write for CapturedLog {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedLog {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn build_service() -> CredentialRefreshService<SqlxConfigRepository> {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("in-memory SQLite URL should be valid");
        let repository = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let resolver = Arc::new(CredentialResolver::new(repository));
        let store = Arc::new(SqlxCredentialStore::new(pool.clone(), pool));
        CredentialRefreshService::new(resolver, store)
    }

    #[tokio::test]
    async fn notification_service_is_installed_once() {
        let service = build_service();
        let first = Arc::new(NotificationService::new());

        service.set_notification_service(Arc::clone(&first));
        service.set_notification_service(Arc::new(NotificationService::new()));

        let installed = service
            .notification_service
            .get()
            .expect("notification service should be installed");
        assert!(Arc::ptr_eq(installed, &first));
    }

    /// `check_and_refresh_source` must not record its `CredentialSource` argument as a
    /// span field; `crate::logging` installs default-format `fmt` layers that prefix
    /// every event with the enclosing span's fields.
    #[tokio::test]
    async fn instrumented_check_does_not_record_credential_material() {
        let captured = CapturedLog::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(captured.clone())
                .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE),
        );
        let _default = tracing::subscriber::set_default(subscriber);

        let source = CredentialSource::new(
            CredentialScope::Platform {
                platform_id: "platform-1".to_string(),
                platform_name: "bilibili".to_string(),
            },
            "SESSDATA=cookie-sentinel".to_string(),
            Some("refresh-sentinel".to_string()),
            "bilibili".to_string(),
        )
        .with_access_token(Some("access-sentinel".to_string()));

        // No manager is registered for "bilibili", so this returns before any I/O while
        // still creating the instrumented span.
        let service = build_service();
        assert!(
            service
                .check_and_refresh_source(&source)
                .await
                .expect("unsupported platform should be skipped")
                .is_none()
        );

        let output = captured.contents();
        // Guards against the negative assertions passing because the span vanished.
        assert!(
            output.contains("check_and_refresh_source"),
            "expected the instrumented span in the captured log: {output}"
        );
        for secret in ["cookie-sentinel", "refresh-sentinel", "access-sentinel"] {
            assert!(
                !output.contains(secret),
                "span fields leaked {secret}: {output}"
            );
        }
    }
}
