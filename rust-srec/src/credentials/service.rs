//! Credential refresh service.
//!
//! Orchestrates credential checking, refreshing, and persistence.

use std::collections::HashMap;
use std::sync::Arc;

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
    notification_service: Option<Arc<NotificationService>>,
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
            notification_service: None,
        }
    }

    /// Wire a NotificationService to emit CredentialEvents as NotificationEvents.
    pub fn set_notification_service(&mut self, service: Arc<NotificationService>) {
        self.notification_service = Some(service);
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
    #[instrument(skip(self), fields(streamer_id = %streamer.id, streamer_name = %streamer.name))]
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
    #[instrument(skip(self), fields(platform = %source.platform_name, scope = %source.scope.describe()))]
    pub async fn check_and_refresh_source(
        &self,
        source: &CredentialSource,
    ) -> Result<Option<String>, CredentialError> {
        // Skip platforms without a registered credential manager (unsupported for auto-refresh).
        if !self.managers.contains_key(&source.platform_name) {
            // debug!(
            //     platform = %source.platform_name,
            //     "Platform does not support credential auto-refresh; skipping"
            // );
            return Ok(None);
        }

        // Check if we already checked today
        if let Some(cached_status) = self.daily_tracker.get_cached_status(&source.scope) {
            return self.handle_cached_status(source, cached_status).await;
        }

        // Acquire lock for this credential scope
        let lock = self.get_refresh_lock(&source.scope);
        let _guard = lock.lock().await;

        // Double-check after acquiring lock (another task may have checked)
        if let Some(cached_status) = self.daily_tracker.get_cached_status(&source.scope) {
            return self.handle_cached_status(source, cached_status).await;
        }

        // First check of the day - call platform API
        self.perform_check_and_refresh(source).await
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
    #[instrument(skip(self), fields(streamer_id = %metadata.id, streamer_name = %metadata.name))]
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
        let Some(service) = self.notification_service.as_ref().cloned() else {
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

        tokio::spawn(async move {
            if let Err(e) = service
                .notify(NotificationEvent::Credential { event })
                .await
            {
                warn!(error = %e, "Failed to dispatch credential notification");
            }
        });
    }

    /// Perform credential refresh.
    #[instrument(skip(self), fields(platform = %source.platform_name, scope = %source.scope.describe()))]
    async fn perform_refresh(
        &self,
        source: &CredentialSource,
    ) -> Result<Option<String>, CredentialError> {
        let manager = self.get_manager(&source.platform_name)?;

        // Check for refresh token
        if !source.has_refresh_token() {
            warn!("Missing refresh_token - cannot auto-refresh");
            let _failure_count = self
                .failure_tracker
                .record_failure(&source.scope, "Missing refresh token");
            return Err(CredentialError::MissingRefreshToken);
        }

        info!("Starting credential refresh");

        let mut state = RefreshState::new(source.cookies.clone(), source.refresh_token.clone());
        // Pass access_token through extra JSON for platform-specific managers.
        if let Some(ref access_token) = source.access_token {
            state.extra = Some(serde_json::json!({
                "access_token": access_token
            }));
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
        self.managers
            .get(platform_name)
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
