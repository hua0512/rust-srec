//! Credential check and failure tracking.
//!
//! - `DailyCheckTracker`: Tracks when each credential scope was last checked (once per day)
//! - `RefreshFailureTracker`: Tracks consecutive failures per credential scope

use std::sync::atomic::{AtomicI32, Ordering};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use dashmap::DashMap;

use super::manager::CredentialStatus;
use super::types::CredentialScope;

/// Tracks when each credential scope was last checked.
///
/// Uses date-based tracking (not timestamp) for "once per day" semantics.
/// For Bilibili, this is sufficient since SESSDATA has ~30 day validity.
pub struct DailyCheckTracker {
    /// Map of scope_key -> last_check_result (for same-day queries)
    cached_results: DashMap<String, CachedCheckResult>,
    /// Used to avoid unbounded growth: we prune stale entries once per day, lazily.
    last_prune_yyyymmdd: AtomicI32,
}

#[derive(Clone)]
struct CachedCheckResult {
    status: CredentialStatus,
    checked_date: NaiveDate,
}

impl DailyCheckTracker {
    /// Create a new daily check tracker.
    pub fn new() -> Self {
        Self {
            cached_results: DashMap::new(),
            last_prune_yyyymmdd: AtomicI32::new(0),
        }
    }

    fn yyyymmdd(date: NaiveDate) -> i32 {
        date.year() * 10_000 + date.month() as i32 * 100 + date.day() as i32
    }

    /// Prune entries that can't ever be returned again (non-today results).
    ///
    /// This keeps memory bounded over long runtimes even if many transient scopes are seen.
    fn prune_if_needed(&self) {
        let today = Utc::now().date_naive();
        let today_key = Self::yyyymmdd(today);

        let last = self.last_prune_yyyymmdd.load(Ordering::Relaxed);
        if last == today_key {
            return;
        }

        if self
            .last_prune_yyyymmdd
            .compare_exchange(last, today_key, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        self.cached_results
            .retain(|_, value| value.checked_date == today);
    }

    /// Check if we already checked today.
    ///
    /// # Returns
    /// - `Some(cached_status)` if already checked today
    /// - `None` if check is needed
    pub fn get_cached_status(&self, scope: &CredentialScope) -> Option<CredentialStatus> {
        self.prune_if_needed();

        let key = scope.cache_key();
        let today = Utc::now().date_naive();

        self.cached_results.get(&key).and_then(|cached| {
            if cached.checked_date == today {
                Some(cached.status.clone())
            } else {
                None
            }
        })
    }

    /// Check if we need to call the API for this credential today.
    ///
    /// # Returns
    /// - `true` if we need to check
    /// - `false` if already checked today
    #[inline]
    pub fn needs_check(&self, scope: &CredentialScope) -> bool {
        self.get_cached_status(scope).is_none()
    }

    /// Record a check result.
    pub fn record_check(&self, scope: &CredentialScope, status: CredentialStatus) {
        self.prune_if_needed();

        let key = scope.cache_key();
        let today = Utc::now().date_naive();

        self.cached_results.insert(
            key,
            CachedCheckResult {
                status,
                checked_date: today,
            },
        );
    }

    /// Force a re-check (e.g., after user manually updates cookies).
    pub fn invalidate(&self, scope: &CredentialScope) {
        let key = scope.cache_key();
        self.cached_results.remove(&key);
    }

    /// Clear all tracked checks (e.g., on startup or midnight rollover).
    pub fn clear_all(&self) {
        self.cached_results.clear();
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.cached_results.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cached_results.is_empty()
    }
}

impl Default for DailyCheckTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks refresh failures per credential scope.
///
/// Used to escalate notifications after consecutive failures.
pub struct RefreshFailureTracker {
    failures: DashMap<String, FailureRecord>,
    /// Lazy pruning guard (avoid unbounded growth from long-dead scopes).
    last_prune_yyyymmdd: AtomicI32,
}

#[derive(Clone)]
struct FailureRecord {
    count: u32,
    first_failure: DateTime<Utc>,
    last_failure: DateTime<Utc>,
    last_error: String,
}

impl RefreshFailureTracker {
    /// Create a new failure tracker.
    pub fn new() -> Self {
        Self {
            failures: DashMap::new(),
            last_prune_yyyymmdd: AtomicI32::new(0),
        }
    }

    fn prune_if_needed(&self) {
        // Keep failure history for a while, but avoid unbounded growth.
        // This is best-effort and runs at most once per day.
        const MAX_AGE_DAYS: i64 = 7;

        let today = Utc::now().date_naive();
        let today_key = DailyCheckTracker::yyyymmdd(today);

        let last = self.last_prune_yyyymmdd.load(Ordering::Relaxed);
        if last == today_key {
            return;
        }

        if self
            .last_prune_yyyymmdd
            .compare_exchange(last, today_key, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let cutoff = Utc::now() - chrono::Duration::days(MAX_AGE_DAYS);
        self.failures
            .retain(|_, record| record.last_failure >= cutoff);
    }

    /// Record a failure and return the updated count.
    pub fn record_failure(&self, scope: &CredentialScope, error: &str) -> u32 {
        self.prune_if_needed();

        let key = scope.cache_key();
        let now = Utc::now();

        let mut entry = self.failures.entry(key).or_insert(FailureRecord {
            count: 0,
            first_failure: now,
            last_failure: now,
            last_error: String::new(),
        });

        entry.count += 1;
        entry.last_failure = now;
        entry.last_error = error.to_string();

        entry.count
    }

    /// Clear failures on success.
    pub fn clear(&self, scope: &CredentialScope) {
        let key = scope.cache_key();
        self.failures.remove(&key);
    }

    /// Get current failure count.
    pub fn failure_count(&self, scope: &CredentialScope) -> u32 {
        self.prune_if_needed();

        let key = scope.cache_key();
        self.failures.get(&key).map(|r| r.count).unwrap_or(0)
    }

    /// Get failure information for a scope.
    pub fn get_failure_info(&self, scope: &CredentialScope) -> Option<FailureInfo> {
        self.prune_if_needed();

        let key = scope.cache_key();
        self.failures.get(&key).map(|r| FailureInfo {
            count: r.count,
            first_failure: r.first_failure,
            last_failure: r.last_failure,
            last_error: r.last_error.clone(),
        })
    }

    /// Clear all tracked failures.
    pub fn clear_all(&self) {
        self.failures.clear();
    }
}

impl Default for RefreshFailureTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about failures for a credential scope.
#[derive(Debug, Clone)]
pub struct FailureInfo {
    /// Number of consecutive failures.
    pub count: u32,
    /// Time of first failure in this sequence.
    pub first_failure: DateTime<Utc>,
    /// Time of most recent failure.
    pub last_failure: DateTime<Utc>,
    /// Error message from last failure.
    pub last_error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daily_check_tracker_records_and_retrieves() {
        let tracker = DailyCheckTracker::new();
        let scope = CredentialScope::Platform {
            platform_id: "platform-bilibili".to_string(),
            platform_name: "bilibili".to_string(),
        };

        // Initially needs check
        assert!(tracker.needs_check(&scope));
        assert!(tracker.get_cached_status(&scope).is_none());

        // Record a check
        tracker.record_check(&scope, CredentialStatus::Valid);

        // Now doesn't need check
        assert!(!tracker.needs_check(&scope));
        assert_eq!(
            tracker.get_cached_status(&scope),
            Some(CredentialStatus::Valid)
        );
    }

    #[test]
    fn test_daily_check_tracker_invalidate() {
        let tracker = DailyCheckTracker::new();
        let scope = CredentialScope::Platform {
            platform_id: "platform-bilibili".to_string(),
            platform_name: "bilibili".to_string(),
        };

        tracker.record_check(&scope, CredentialStatus::Valid);
        assert!(!tracker.needs_check(&scope));

        // Invalidate
        tracker.invalidate(&scope);
        assert!(tracker.needs_check(&scope));
    }

    #[test]
    fn test_failure_tracker_counts() {
        let tracker = RefreshFailureTracker::new();
        let scope = CredentialScope::Platform {
            platform_id: "platform-bilibili".to_string(),
            platform_name: "bilibili".to_string(),
        };

        assert_eq!(tracker.failure_count(&scope), 0);

        tracker.record_failure(&scope, "Network error");
        assert_eq!(tracker.failure_count(&scope), 1);

        tracker.record_failure(&scope, "Network error again");
        assert_eq!(tracker.failure_count(&scope), 2);

        // Clear on success
        tracker.clear(&scope);
        assert_eq!(tracker.failure_count(&scope), 0);
    }
}
