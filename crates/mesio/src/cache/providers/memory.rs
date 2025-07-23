//! # Memory Cache Provider
//!
//! This module provides an in-memory cache implementation using Moka caching.

use std::time::Duration;

use bytes::Bytes;
use moka::future::Cache as MokaCache;
use tracing::{debug, warn};

use crate::cache::providers::CacheProvider;
use crate::cache::types::{CacheKey, CacheLookupResult, CacheMetadata, CacheResult, CacheStatus};

/// Entry in the memory cache
#[derive(Clone)]
struct CacheEntry {
    /// Cached data bytes
    data: Bytes,
    /// Metadata for the cached content
    metadata: CacheMetadata,
}

/// Memory cache provider implementation using Moka
#[derive(Clone)]
pub struct MemoryCache {
    /// Moka cache for storing entries
    cache: MokaCache<CacheKey, CacheEntry>,
    /// Maximum size for this cache in bytes
    max_size: u64,
}

impl MemoryCache {
    /// Create a new memory cache with the specified size limit
    pub fn new(max_size_bytes: u64, ttl_seconds: u64) -> Self {
        if max_size_bytes == 0 {
            panic!("Memory cache size must be greater than zero");
        }

        // Size based eviction
        let mut builder = MokaCache::builder()
            .weigher(|_k, v: &CacheEntry| v.data.len().try_into().unwrap_or(u32::MAX))
            .max_capacity(max_size_bytes);

        // Only add TTL if it's non-zero
        if ttl_seconds > 0 {
            builder = builder.time_to_live(Duration::from_secs(ttl_seconds));
        }

        // Build the cache
        let cache = builder.build();

        debug!(
            max_size = max_size_bytes,
            ttl_seconds = ttl_seconds,
            "Memory cache created with size limit and TTL"
        );

        Self {
            cache,
            max_size: max_size_bytes,
        }
    }
}

#[async_trait::async_trait]
impl CacheProvider for MemoryCache {
    async fn contains(&self, key: &CacheKey) -> CacheResult<bool> {
        Ok(self.cache.contains_key(key))
    }

    async fn get(&self, key: &CacheKey) -> CacheLookupResult {
        // Try to get the entry from the cache
        if let Some(entry) = self.cache.get(key).await {
            let data = entry.data.clone();
            let metadata = entry.metadata.clone();

            // Manually check for expiration based on metadata, which is necessary when no
            // global TTL is set on the Moka cache.
            if let Some(expires_at) = metadata.expires_at {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                if now > 0 && now >= expires_at {
                    debug!(
                        key = ?key,
                        "Memory cache entry expired based on metadata.expires_at"
                    );
                    self.cache.invalidate(key).await;
                    return Ok(Some((data, metadata, CacheStatus::Expired)));
                }
            }

            // Return a cache hit
            return Ok(Some((data, metadata, CacheStatus::Hit)));
        }

        // Cache miss
        Ok(None)
    }

    async fn put(&self, key: CacheKey, data: Bytes, metadata: CacheMetadata) -> CacheResult<()> {
        let size = metadata.size;

        // Check if this entry is too large to be cached at all
        // A single entry shouldn't be larger than the total cache size
        if size > self.max_size {
            warn!(
                key = ?key,
                size = size,
                max_size = self.max_size,
                "Entry too large for memory cache, skipping"
            );
            return Ok(());
        }

        // Add the new entry
        let entry = CacheEntry {
            data,
            metadata: metadata.clone(),
        };

        // Insert the entry into the cache
        // Note: Moka handles TTL expiration internally based on the expires_at field in metadata
        self.cache.insert(key, entry).await;

        // debug!(
        //     key = ?key,
        //     size = size,
        //     max_size = self.max_size,
        //     "Added entry to memory cache"
        // );

        Ok(())
    }

    async fn remove(&self, key: &CacheKey) -> CacheResult<()> {
        // Check if entry exists before removing
        if self.cache.get(key).await.is_some() {
            // Remove from cache
            self.cache.invalidate(key).await;
            debug!(key = ?key, "Removed entry from memory cache");
        }

        Ok(())
    }

    async fn clear(&self) -> CacheResult<()> {
        // Clear the cache
        self.cache.invalidate_all();

        debug!("Memory cache cleared");
        Ok(())
    }

    async fn sweep(&self) -> CacheResult<()> {
        // Moka handles expiration automatically based on its configuration (time_to_live, time_to_idle).
        // `run_pending_tasks` can be called to eagerly perform maintenance, like removing expired entries.
        self.cache.run_pending_tasks().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::types::{CacheKey, CacheMetadata, CacheResourceType, CacheStatus};
    use bytes::Bytes;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::time::sleep;

    #[inline]
    pub fn init_tracing() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer() // Write to test output
            .try_init();
    }

    // Helper to create a CacheKey
    fn key(name: &str) -> CacheKey {
        CacheKey::new(CacheResourceType::Content, name.to_string(), None)
    }

    // Helper to create Bytes data
    fn data(content: &str) -> Bytes {
        Bytes::from(content.to_string())
    }

    // Helper to create CacheMetadata
    fn metadata(size: u64, expires_in_secs: Option<u64>) -> CacheMetadata {
        let mut meta = CacheMetadata::new(size)
            .with_content_type_option(Some("application/octet-stream".to_string()));
        if let Some(ttl_secs) = expires_in_secs {
            meta = meta.with_expiration(Duration::from_secs(ttl_secs));
        }
        meta
    }

    // Helper to create CacheMetadata that is already expired
    fn expired_metadata(size: u64) -> CacheMetadata {
        let mut meta = CacheMetadata::new(size)
            .with_content_type_option(Some("application/octet-stream".to_string()));
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        meta.cached_at = now_secs.saturating_sub(1000);
        meta.expires_at = Some(now_secs.saturating_sub(500));
        meta
    }

    #[tokio::test]
    async fn test_new_cache_valid_params() {
        let cache = MemoryCache::new(1024 * 1024, 60);
        assert_eq!(cache.max_size, 1024 * 1024);
    }

    #[tokio::test]
    async fn test_new_cache_no_ttl() {
        let cache = MemoryCache::new(1024 * 1024, 0);
        assert_eq!(cache.max_size, 1024 * 1024);
    }

    #[tokio::test]
    #[should_panic(expected = "Memory cache size must be greater than zero")]
    async fn test_new_cache_zero_size_panics() {
        MemoryCache::new(0, 60);
    }

    #[tokio::test]
    async fn test_put_get_hit() {
        let cache = MemoryCache::new(100, 60);
        let k = key("item1");
        let d = data("hello");
        let m = metadata(d.len() as u64, Some(60));

        cache.put(k.clone(), d.clone(), m.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle after put

        let result = cache.get(&k).await.unwrap();
        match result {
            Some((res_d, res_m, status)) => {
                assert_eq!(res_d, d);
                assert_eq!(res_m.size, m.size);
                assert_eq!(res_m.expires_at, m.expires_at);
                assert_eq!(res_m.content_type, m.content_type);
                assert_eq!(status, CacheStatus::Hit);
            }
            None => panic!("Expected CacheHit, got None"),
        }
    }

    #[tokio::test]
    async fn test_get_miss() {
        let cache = MemoryCache::new(100, 60);
        let k = key("non_existent");
        let result = cache.get(&k).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_contains_key() {
        let cache = MemoryCache::new(100, 60);
        let k = key("item_contains");
        let d = data("hello");
        let m = metadata(d.len() as u64, Some(60));

        assert!(!cache.contains(&k).await.unwrap());
        cache.put(k.clone(), d, m).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle after put
        assert!(cache.contains(&k).await.unwrap());
    }

    #[tokio::test]
    async fn test_put_get_expired_by_metadata() {
        let cache = MemoryCache::new(100, 3600);
        let k = key("expired_item_meta");
        let d = data("stale_data");
        let m_expired = expired_metadata(d.len() as u64);

        cache
            .put(k.clone(), d.clone(), m_expired.clone())
            .await
            .unwrap();
        cache.cache.run_pending_tasks().await; // Settle after put

        let result = cache.get(&k).await.unwrap();
        match result {
            Some((res_d, res_m, status)) => {
                assert_eq!(res_d, d);
                assert_eq!(res_m.size, m_expired.size);
                assert_eq!(status, CacheStatus::Expired);
            }
            None => panic!("Expected CacheExpired, got None"),
        }

        cache.cache.run_pending_tasks().await; // Ensure invalidation from get() is processed
        assert!(
            !cache.contains(&k).await.unwrap(),
            "Expired item should be removed after get"
        );
        let result_after_expiry_get = cache.get(&k).await.unwrap();
        assert!(
            result_after_expiry_get.is_none(),
            "Item should be None after being fetched as expired"
        );
    }

    #[tokio::test]
    async fn test_put_get_expired_by_moka_ttl() {
        let cache = MemoryCache::new(100, 1);
        let k = key("short_lived_item_moka");
        let d = data("transient");
        let m = metadata(d.len() as u64, Some(3600));

        cache.put(k.clone(), d.clone(), m.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle after put

        assert!(cache.contains(&k).await.unwrap());
        match cache.get(&k).await.unwrap() {
            Some((_, _, CacheStatus::Hit)) => {}
            res => panic!("Expected Hit initially, got {res:?}"),
        }

        sleep(Duration::from_millis(1500)).await;
        cache.cache.run_pending_tasks().await; // Explicitly run Moka's maintenance for TTL

        let result = cache.get(&k).await.unwrap();
        assert!(
            result.is_none(),
            "Item should be None due to Moka TTL expiry"
        );
        assert!(
            !cache.contains(&k).await.unwrap(),
            "Item should not be contained due to Moka TTL expiry"
        );
    }

    #[tokio::test]
    async fn test_put_too_large_entry() {
        let cache = MemoryCache::new(50, 60);
        let k = key("large_item");
        let d =
            data("This string is definitely longer than fifty bytes, so it should not be cached.");
        let m = metadata(d.len() as u64, Some(60));

        assert!(d.len() as u64 > cache.max_size);

        cache.put(k.clone(), d, m).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle

        assert!(!cache.contains(&k).await.unwrap());
        let result = cache.get(&k).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_remove_key() {
        let cache = MemoryCache::new(100, 60);
        let k = key("item_to_remove");
        let d = data("content");
        let m = metadata(d.len() as u64, Some(60));

        cache.put(k.clone(), d, m).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        assert!(cache.contains(&k).await.unwrap());

        cache.remove(&k).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        assert!(!cache.contains(&k).await.unwrap());
        let result = cache.get(&k).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_remove_non_existent_key() {
        let cache = MemoryCache::new(100, 60);
        let k = key("ghost_key");
        let result = cache.remove(&k).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let cache = MemoryCache::new(100, 60);
        let k1 = key("item_clear1");
        let d1 = data("data1_clear");
        let m1 = metadata(d1.len() as u64, Some(60));
        let k2 = key("item_clear2");
        let d2 = data("data2_clear");
        let m2 = metadata(d2.len() as u64, Some(60));

        cache.put(k1.clone(), d1, m1).await.unwrap();
        cache.put(k2.clone(), d2, m2).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle puts

        assert!(cache.contains(&k1).await.unwrap());
        assert!(cache.contains(&k2).await.unwrap());

        cache.clear().await.unwrap();
        cache.cache.run_pending_tasks().await; // Ensure Moka processes invalidations

        assert!(!cache.contains(&k1).await.unwrap());
        assert!(!cache.contains(&k2).await.unwrap());
        assert_eq!(cache.cache.entry_count(), 0);
    }

    #[tokio::test]
    async fn test_sweep_runs_and_clears_moka_expired() {
        let cache = MemoryCache::new(100, 1);
        let k = key("sweep_item");
        let d = data("sweep_data");
        let m = metadata(d.len() as u64, Some(3600));

        cache.put(k.clone(), d, m).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle put
        assert!(cache.contains(&k).await.unwrap());

        sleep(Duration::from_millis(1500)).await;

        cache.sweep().await.unwrap(); // This calls Moka's run_pending_tasks

        let result = cache.get(&k).await.unwrap();
        assert!(
            result.is_none(),
            "Item should be None after Moka TTL expiry and sweep"
        );
        assert!(!cache.contains(&k).await.unwrap());
    }
    #[tokio::test]
    async fn test_eviction_on_max_size() {
        init_tracing();
        let cache = MemoryCache::new(10, 60); // Max size 10 bytes. Each item 5 bytes.

        let k1 = key("evict_item1");
        let d1 = data("dataA"); // 5 bytes
        let m1 = metadata(d1.len() as u64, Some(60));

        let k2 = key("evict_item2");
        let d2 = data("dataB"); // 5 bytes
        let m2 = metadata(d2.len() as u64, Some(60));

        let k3 = key("evict_item3");
        let d3 = data("dataC"); // 5 bytes
        let m3 = metadata(d3.len() as u64, Some(60));
        debug!("Putting k1 (evict_item1)");

        cache.cache.run_pending_tasks().await; // Settle
        debug!(
            "After putting k1: contains_k1={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );
        debug!("Putting k2 (evict_item2)");
        // Put first item
        cache.put(k1.clone(), d1.clone(), m1.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        debug!(
            "After putting k2: contains_k1={}, contains_k2={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );

        // Put second item
        debug!("Cache should be full now (k1, k2). Sweeping...");
        cache.put(k2.clone(), d2.clone(), m2.clone()).await.unwrap();
        debug!(
            "After sweep1: contains_k1={}, contains_k2={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );

        debug!("Putting k3 (evict_item3) - expecting eviction");
        // At this point, both k1 and k2 are in the cache, total size is 10 bytes.
        cache.cache.run_pending_tasks().await; // Settle
        debug!(
            "After putting k3: contains_k1={}, contains_k2={}, contains_k3={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.contains(&k3).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );
        debug!("Sweeping after k3 put...");

        debug!(
            "After sweep2 (final state before assertions): contains_k1={}, contains_k2={}, contains_k3={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.contains(&k3).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );
        cache.sweep().await.unwrap(); // Ensure Moka processes invalidations

        // Put third item - this should trigger eviction
        cache.put(k3.clone(), d3.clone(), m3.clone()).await.unwrap();

        cache.sweep().await.unwrap(); // Ensure Moka processes invalidations

        // After all three puts, K3 is evicted
        // Moka cache, used by MemoryCache, employs a TinyLFU eviction policy by default. With TinyLFU,
        // the newly added item (k3) was being evicted because it was considered less valuable (due to lower initial frequency estimates)
        // than the existing items, even though k1 was technically the LRU item among k1 and k2.
        assert!(
            cache.contains(&k1).await.unwrap(),
            "k1 should still be present as k3 was evicted"
        );
        assert!(
            cache.contains(&k2).await.unwrap(),
            "k2 should still be present as k3 was evicted"
        );
        assert!(
            !cache.contains(&k3).await.unwrap(),
            "k3 (newest) should have been evicted by TinyLFU"
        );
        assert_eq!(
            cache.cache.weighted_size(),
            10,
            "Cache size should be 10 (k1+k2) after k3's eviction"
        );
        assert_eq!(
            cache.cache.entry_count(),
            2,
            "Entry count should be 2 (k1, k2)"
        );
    }

    #[tokio::test]
    async fn test_eviction_respects_access_order_lru_like() {
        init_tracing();
        let cache = MemoryCache::new(10, 60);

        let k1 = key("lru_item1");
        let d1 = data("lruA");
        let m1 = metadata(d1.len() as u64, Some(60));

        let k2 = key("lru_item2");
        let d2 = data("lruB");
        let m2 = metadata(d2.len() as u64, Some(60));

        let k3 = key("lru_item3");
        let d3 = data("lruC");
        let m3 = metadata(d3.len() as u64, Some(60));

        cache.put(k1.clone(), d1.clone(), m1.clone()).await.unwrap();
        cache.put(k2.clone(), d2.clone(), m2.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle initial puts
        debug!(
            "After initial puts (k1, k2): contains_k1={}, contains_k2={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );

        // Access k1 to make it more recently used.
        assert!(
            cache.get(&k1).await.unwrap().is_some(),
            "k1 should be gettable"
        );

        debug!(
            "After getting k1 and running pending tasks: contains_k1={}, contains_k2={}, size={}, count={}",
            cache.contains(&k1).await.unwrap(),
            cache.contains(&k2).await.unwrap(),
            cache.cache.weighted_size(),
            cache.cache.entry_count()
        );
        // Put k3. This should be rejected by admission policy, as k1 is more recently used.
        cache.put(k3.clone(), d3.clone(), m3.clone()).await.unwrap();
        // Run pending tasks after the final put to settle the eviction.
        cache.cache.run_pending_tasks().await;

        assert!(
            cache.contains(&k1).await.unwrap(),
            "k1 should still be present (accessed)"
        );
        assert!(
            cache.contains(&k2).await.unwrap(),
            "k2 should still be present (k3 was not admitted)"
        );
        assert!(
            !cache.contains(&k3).await.unwrap(),
            "k3 should be absent (rejected by admission)"
        );
        assert_eq!(cache.cache.weighted_size(), 8, "Size should be 8 (k1+k2)");
        assert_eq!(cache.cache.entry_count(), 2, "Count should be 2 (k1, k2)");
    }

    #[tokio::test]
    async fn test_double_put_updates_value_and_weight() {
        let cache = MemoryCache::new(100, 60);
        let k = key("item_double_put");

        let d1 = data("value1");
        let m1 = metadata(d1.len() as u64, Some(60));

        let d2 = data("new_val");
        let m2 = metadata(d2.len() as u64, Some(60));

        cache.put(k.clone(), d1.clone(), m1.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        let result1 = cache.get(&k).await.unwrap().expect("Item after first put");
        assert_eq!(result1.0, d1);
        assert_eq!(result1.1.size, m1.size);
        assert_eq!(cache.cache.weighted_size(), d1.len() as u64);

        cache.put(k.clone(), d2.clone(), m2.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        let result2 = cache.get(&k).await.unwrap().expect("Item after second put");
        assert_eq!(result2.0, d2, "Data should be updated");
        assert_eq!(result2.1.size, m2.size, "Metadata size should be updated");
        assert_eq!(result2.2, CacheStatus::Hit);
        assert_eq!(
            cache.cache.weighted_size(),
            d2.len() as u64,
            "Cache weight should reflect new size"
        );
        assert_eq!(cache.cache.entry_count(), 1);
    }

    #[tokio::test]
    async fn test_put_get_no_metadata_expiry_no_moka_ttl() {
        let cache = MemoryCache::new(100, 0);
        let k = key("item_no_expiry");
        let d = data("permanent_data");
        let m = metadata(d.len() as u64, None);

        cache.put(k.clone(), d.clone(), m.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle
        sleep(Duration::from_millis(50)).await;

        let result = cache.get(&k).await.unwrap();
        match result {
            Some((res_d, res_m, status)) => {
                assert_eq!(res_d, d);
                assert!(res_m.expires_at.is_none());
                assert_eq!(status, CacheStatus::Hit);
            }
            None => panic!("Expected CacheHit, got None for non-expiring item"),
        }
        assert!(cache.contains(&k).await.unwrap());
    }

    #[tokio::test]
    async fn test_put_get_no_moka_ttl_with_metadata_expiry_logic() {
        let cache = MemoryCache::new(100, 0);
        let k = key("item_meta_expiry_only");
        let d = data("data_expires_by_meta");
        let m = metadata(d.len() as u64, Some(1));

        cache.put(k.clone(), d.clone(), m.clone()).await.unwrap();
        cache.cache.run_pending_tasks().await; // Settle

        match cache.get(&k).await.unwrap() {
            Some((_, _, CacheStatus::Hit)) => {}
            res => panic!("Expected Hit initially, got {res:?}"),
        }

        sleep(Duration::from_millis(1500)).await;
        cache.cache.run_pending_tasks().await; // Ensure expiry check in get() can lead to invalidation processing

        let result = cache.get(&k).await.unwrap();
        match result {
            Some((res_d, _, CacheStatus::Expired)) => {
                assert_eq!(res_d, d);
            }
            res => panic!("Expected CacheExpired due to metadata, got {res:?}"),
        }

        cache.cache.run_pending_tasks().await; // Ensure invalidation from get() is processed
        assert!(
            !cache.contains(&k).await.unwrap(),
            "Item should be removed after being fetched as expired"
        );
        assert!(
            cache.get(&k).await.unwrap().is_none(),
            "Subsequent get should be None"
        );
    }
}
