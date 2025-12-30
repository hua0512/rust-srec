//! Configuration cache implementation.
//!
//! This module provides a thread-safe cache for merged configurations
//! with TTL-based eviction and request deduplication.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, OnceCell};

use crate::domain::config::ResolvedStreamerContext;

/// Default TTL for cached configurations (1 hour).
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// A cached configuration entry with expiration time.
#[derive(Clone)]
struct CacheEntry {
    context: Arc<ResolvedStreamerContext>,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(context: Arc<ResolvedStreamerContext>, ttl: Duration) -> Self {
        Self {
            context,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// In-flight request tracker for deduplication.
///
/// When multiple requests come in for the same streamer config simultaneously,
/// only one will actually resolve the config while others wait for the result.
pub(super) type InFlightResult = std::result::Result<Arc<ResolvedStreamerContext>, String>;

pub(super) struct InFlightState {
    result: OnceCell<InFlightResult>,
    notify: Notify,
}

impl InFlightState {
    fn new() -> Self {
        Self {
            result: OnceCell::new(),
            notify: Notify::new(),
        }
    }

    fn set_result(&self, result: InFlightResult) {
        let _ = self.result.set(result);
        self.notify.notify_waiters();
    }

    async fn wait(&self) -> InFlightResult {
        loop {
            if let Some(result) = self.result.get() {
                return result.clone();
            }

            let notified = self.notify.notified();
            if let Some(result) = self.result.get() {
                return result.clone();
            }

            notified.await;
        }
    }
}

pub(super) type InFlightRequest = Arc<InFlightState>;

/// Thread-safe cache for merged configurations.
///
/// Uses DashMap for concurrent access and supports TTL-based eviction.
/// Also provides request deduplication to prevent duplicate config resolution.
#[derive(Clone)]
pub struct ConfigCache {
    /// Cache for streamer merged configs.
    streamer_configs: Arc<DashMap<String, CacheEntry>>,
    /// In-flight requests for deduplication.
    in_flight: Arc<DashMap<String, InFlightRequest>>,
    /// TTL for cache entries.
    ttl: Duration,
}

impl ConfigCache {
    /// Create a new cache with default TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// Create a new cache with specified TTL.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            streamer_configs: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Get a cached configuration for a streamer.
    ///
    /// Returns None if not cached or expired.
    pub fn get(&self, streamer_id: &str) -> Option<Arc<ResolvedStreamerContext>> {
        let entry = self.streamer_configs.get(streamer_id)?;

        if entry.is_expired() {
            drop(entry); // Release the lock before removing
            self.streamer_configs.remove(streamer_id);
            return None;
        }

        Some(entry.context.clone())
    }

    /// Insert a configuration into the cache.
    pub fn insert(&self, streamer_id: String, context: Arc<ResolvedStreamerContext>) {
        let entry = CacheEntry::new(context, self.ttl);
        self.streamer_configs.insert(streamer_id, entry);
    }

    /// Remove a specific streamer's configuration from the cache.
    pub fn invalidate(&self, streamer_id: &str) {
        self.streamer_configs.remove(streamer_id);
        self.cancel_in_flight(
            streamer_id,
            format!("Configuration invalidated for streamer {streamer_id}"),
        );
    }

    /// Invalidate all cached configurations.
    pub fn invalidate_all(&self) {
        let reason = "Configuration cache invalidated".to_string();
        for entry in self.in_flight.iter() {
            entry.value().set_result(Err(reason.clone()));
        }
        self.streamer_configs.clear();
        self.in_flight.clear();
    }

    /// Invalidate configurations for streamers matching a predicate.
    pub fn invalidate_where<F>(&self, predicate: F)
    where
        F: Fn(&str) -> bool,
    {
        self.streamer_configs.retain(|key, _| !predicate(key));
        let reason = "Configuration cache invalidated".to_string();
        self.in_flight.retain(|key, request| {
            if predicate(key) {
                request.set_result(Err(reason.clone()));
                false
            } else {
                true
            }
        });
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.streamer_configs.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.streamer_configs.is_empty()
    }

    /// Remove all expired entries from the cache.
    pub fn cleanup_expired(&self) -> usize {
        let before = self.len();
        self.streamer_configs.retain(|_, entry| !entry.is_expired());
        before - self.len()
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.len(),
            in_flight_count: self.in_flight.len(),
            ttl: self.ttl,
        }
    }

    // ========== Request Deduplication ==========

    /// Get or create an in-flight request for a streamer.
    ///
    /// This is used to deduplicate concurrent requests for the same streamer's config.
    /// Returns (OnceCell, is_new) where is_new indicates if this is a new request.
    pub(super) fn get_or_create_in_flight(&self, streamer_id: &str) -> (InFlightRequest, bool) {
        // Try to get existing in-flight request
        if let Some(existing) = self.in_flight.get(streamer_id) {
            return (existing.clone(), false);
        }

        // Create new in-flight request
        let request = Arc::new(InFlightState::new());

        // Use entry API to handle race condition
        match self.in_flight.entry(streamer_id.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                // Another thread beat us to it
                (entry.get().clone(), false)
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(request.clone());
                (request, true)
            }
        }
    }

    /// Complete an in-flight request by setting its result.
    ///
    /// This will also cache the result and remove the in-flight entry.
    pub(super) fn complete_in_flight(
        &self,
        streamer_id: &str,
        request: &InFlightRequest,
        context: Arc<ResolvedStreamerContext>,
    ) {
        if let Some(current) = self.in_flight.get(streamer_id) {
            if !Arc::ptr_eq(&current, request) {
                return;
            }
        } else {
            return;
        }

        if let Some((_, current)) = self.in_flight.remove(streamer_id) {
            if Arc::ptr_eq(&current, request) {
                current.set_result(Ok(context.clone()));
                self.insert(streamer_id.to_string(), context);
            } else {
                self.in_flight.insert(streamer_id.to_string(), current);
            }
        }
    }

    /// Fail an in-flight request, waking any waiters with an error message.
    pub(super) fn fail_in_flight(
        &self,
        streamer_id: &str,
        request: &InFlightRequest,
        reason: String,
    ) {
        if let Some(current) = self.in_flight.get(streamer_id) {
            if !Arc::ptr_eq(&current, request) {
                return;
            }
        } else {
            return;
        }

        if let Some((_, current)) = self.in_flight.remove(streamer_id) {
            if Arc::ptr_eq(&current, request) {
                current.set_result(Err(reason));
            } else {
                self.in_flight.insert(streamer_id.to_string(), current);
            }
        }
    }

    /// Cancel an in-flight request, waking any waiters with an error message.
    fn cancel_in_flight(&self, streamer_id: &str, reason: String) {
        if let Some((_, request)) = self.in_flight.remove(streamer_id) {
            request.set_result(Err(reason));
        }
    }

    /// Wait for an in-flight request to complete.
    ///
    /// Returns the config once it's available.
    pub(super) async fn wait_for_in_flight(&self, cell: &InFlightRequest) -> InFlightResult {
        cell.wait().await
    }

    /// Check if there's an in-flight request for a streamer.
    pub fn has_in_flight(&self, streamer_id: &str) -> bool {
        self.in_flight.contains_key(streamer_id)
    }

    /// Get the number of in-flight requests.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

impl Default for ConfigCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the configuration cache.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries in the cache.
    pub entry_count: usize,
    /// Number of in-flight requests.
    pub in_flight_count: usize,
    /// TTL for cache entries.
    pub ttl: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProxyConfig;
    use crate::domain::config::MergedConfig;

    fn create_test_context() -> ResolvedStreamerContext {
        let config = MergedConfig::builder()
            .with_global(
                "/app/output".to_string(),
                "{streamer}-{title}".to_string(),
                "flv".to_string(),
                1024,
                0,
                8589934592,
                false,
                ProxyConfig::disabled(),
                "ffmpeg".to_string(),
                300,  // session_gap_time_secs
                None, // pipeline
                None, // session_complete_pipeline
                None, // paired_segment_pipeline
                true,
            )
            .with_platform(
                Some(60000),
                Some(1000),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None, // pipeline
                None, // session_complete_pipeline
                None, // paired_segment_pipeline
            )
            .build();

        ResolvedStreamerContext {
            config: Arc::new(config),
            credential_source: None,
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = ConfigCache::new();
        let context = create_test_context();

        cache.insert("streamer-1".to_string(), Arc::new(context));

        let cached = cache.get("streamer-1");
        assert!(cached.is_some());
        assert_eq!(
            cached.unwrap().config.output_folder,
            "/app/output".to_string()
        );
    }

    #[test]
    fn test_cache_miss() {
        let cache = ConfigCache::new();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let cache = ConfigCache::new();
        cache.insert("streamer-1".to_string(), Arc::new(create_test_context()));

        assert!(cache.get("streamer-1").is_some());

        cache.invalidate("streamer-1");

        assert!(cache.get("streamer-1").is_none());
    }

    #[test]
    fn test_cache_invalidate_all() {
        let cache = ConfigCache::new();
        cache.insert("streamer-1".to_string(), Arc::new(create_test_context()));
        cache.insert("streamer-2".to_string(), Arc::new(create_test_context()));

        assert_eq!(cache.len(), 2);

        cache.invalidate_all();

        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_ttl_expiration() {
        let cache = ConfigCache::with_ttl(Duration::from_millis(10));
        cache.insert("streamer-1".to_string(), Arc::new(create_test_context()));

        // Should be present immediately
        assert!(cache.get("streamer-1").is_some());

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(20));

        // Should be expired now
        assert!(cache.get("streamer-1").is_none());
    }

    #[test]
    fn test_cache_invalidate_where() {
        let cache = ConfigCache::new();
        cache.insert(
            "platform-a-streamer-1".to_string(),
            Arc::new(create_test_context()),
        );
        cache.insert(
            "platform-a-streamer-2".to_string(),
            Arc::new(create_test_context()),
        );
        cache.insert(
            "platform-b-streamer-1".to_string(),
            Arc::new(create_test_context()),
        );

        assert_eq!(cache.len(), 3);

        // Invalidate all platform-a streamers
        cache.invalidate_where(|key| key.starts_with("platform-a"));

        assert_eq!(cache.len(), 1);
        assert!(cache.get("platform-b-streamer-1").is_some());
    }

    #[test]
    fn test_cache_cleanup_expired() {
        let cache = ConfigCache::with_ttl(Duration::from_millis(10));
        cache.insert("streamer-1".to_string(), Arc::new(create_test_context()));
        cache.insert("streamer-2".to_string(), Arc::new(create_test_context()));

        std::thread::sleep(Duration::from_millis(20));

        let removed = cache.cleanup_expired();
        assert_eq!(removed, 2);
        assert!(cache.is_empty());
    }
}
