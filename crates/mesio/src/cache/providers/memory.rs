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
        // Size based eviction
        let mut builder = MokaCache::builder()
            .weigher(|_k, v: &CacheEntry| v.data.len().try_into().unwrap_or(u32::MAX));

        if max_size_bytes == 0 {
            warn!("Memory cache disabled (max_size_bytes = 0)");
        } else {
            builder = builder.max_capacity(max_size_bytes);
        }

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
            if metadata.is_expired() {
                debug!(
                    key = ?key,
                    "Memory cache entry expired based on metadata.expires_at"
                );
                self.cache.invalidate(key).await;
                return Ok(Some((data, metadata, CacheStatus::Expired)));
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
        let entry = CacheEntry { data, metadata };

        self.cache.insert(key, entry).await;

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
    async fn test_new_cache_zero_size_disables_cache() {
        let cache = MemoryCache::new(0, 60);
        assert_eq!(cache.max_size, 0);

        let k = key("disabled_item");
        let d = data("hello");
        let m = metadata(d.len() as u64, Some(60));

        cache.put(k.clone(), d, m).await.unwrap();
        cache.cache.run_pending_tasks().await;

        // With max_size=0, puts are always skipped.
        assert!(cache.get(&k).await.unwrap().is_none());
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
}
