//! # Cache Manager
//!
//! This module provides the main cache manager that coordinates between memory and file caches.

use std::sync::Arc;

use bytes::Bytes;
use tokio::io;

use crate::cache::providers::file::FileCache;
use crate::cache::providers::memory::MemoryCache;
use crate::cache::providers::provider::CacheProvider;
use crate::cache::types::{
    CacheConfig, CacheKey, CacheLookupResult, CacheMetadata, CacheResourceType, CacheResult,
};

/// Cache manager handling both memory and file caching

#[derive(Clone)]
pub struct CacheManager {
    memory_cache: Arc<MemoryCache>,
    file_cache: Arc<FileCache>,
    config: Arc<CacheConfig>,
}

impl CacheManager {
    /// Create a new cache manager with the specified configuration
    pub async fn new(mut config: CacheConfig) -> io::Result<Self> {
        // If no disk cache path provided, use system temp
        if config.disk_cache_path.is_none() {
            let temp_dir = std::env::temp_dir();
            config.disk_cache_path = Some(temp_dir.join("mesio-cache"));
        }

        let cache_dir = config.disk_cache_path.as_ref().unwrap().clone();
        let config = Arc::new(config);

        // Create memory cache with configured size
        let memory_cache = Arc::new(MemoryCache::new(config.max_memory_cache_size, 0));

        // Create file cache with configured directory
        let file_cache = Arc::new(FileCache::new(
            cache_dir,
            config.max_disk_cache_size > 0 && config.enabled,
        ));

        // Initialize the cache directories in advance
        if config.enabled {
            file_cache.ensure_initialized().await?;
        }

        Ok(Self {
            memory_cache,
            file_cache,
            config,
        })
    }

    /// Create a new cache manager from an existing configuration
    pub fn from_config(config: CacheConfig) -> io::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        runtime.block_on(Self::new(config))
    }

    /// Get a value from the cache
    pub async fn get(&self, key: &CacheKey) -> CacheLookupResult {
        if !self.config.enabled {
            return Ok(None);
        }

        // Check memory cache first
        if let Some((data, metadata, status)) = self.memory_cache.get(key).await? {
            return Ok(Some((data, metadata, status)));
        }

        // Try file cache if memory cache misses
        if let Some((data, metadata, status)) = self.file_cache.get(key).await? {
            // Store in memory cache for faster access next time
            let _ = self
                .memory_cache
                .put(key.clone(), data.clone(), metadata.clone())
                .await;

            return Ok(Some((data, metadata, status)));
        }

        Ok(None)
    }

    /// Put a value in the cache
    pub async fn put(
        &self,
        key: CacheKey,
        data: Bytes,
        metadata: CacheMetadata,
    ) -> CacheResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Store in memory cache
        let _ = self
            .memory_cache
            .put(key.clone(), data.clone(), metadata.clone())
            .await;

        // Store in file cache
        self.file_cache.put(key, data, metadata).await
    }

    /// Remove a key from cache
    pub async fn remove(&self, key: &CacheKey) -> CacheResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Remove from both caches
        let mem_result = self.memory_cache.remove(key).await;
        let file_result = self.file_cache.remove(key).await;

        // Return file cache error if any, otherwise memory cache error if any
        file_result.or(mem_result)
    }

    /// Clear all entries
    pub async fn clear(&self) -> CacheResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Clear both caches
        let mem_result = self.memory_cache.clear().await;
        let file_result = self.file_cache.clear().await;

        // Return file cache error if any, otherwise memory cache error if any
        file_result.or(mem_result)
    }

    /// Check if a key exists in the cache
    pub async fn contains(&self, key: &CacheKey) -> CacheResult<bool> {
        if !self.config.enabled {
            return Ok(false);
        }

        // Check memory cache first
        if self.memory_cache.contains(key).await? {
            return Ok(true);
        }

        // Check file cache if not in memory
        self.file_cache.contains(key).await
    }

    // Convenience methods for common operations

    /// Get a cached HTTP response
    pub async fn get_response(&self, url: &str) -> CacheLookupResult {
        let key = CacheKey::new(CacheResourceType::Response, url, None);
        self.get(&key).await
    }

    /// Cache an HTTP response
    pub async fn put_response(
        &self,
        url: &str,
        data: Bytes,
        etag: Option<String>,
        last_modified: Option<String>,
        content_type: Option<String>,
    ) -> CacheResult<()> {
        let key = CacheKey::new(CacheResourceType::Response, url, None);
        let mut metadata =
            CacheMetadata::new(data.len() as u64).with_expiration(self.config.default_ttl);

        if let Some(etag) = etag {
            metadata = metadata.with_etag(etag);
        }

        if let Some(last_modified) = last_modified {
            metadata = metadata.with_last_modified(last_modified);
        }

        if let Some(content_type) = content_type {
            metadata = metadata.with_content_type(content_type);
        }

        self.put(key, data, metadata).await
    }

    /// Get cached segment data
    pub async fn get_segment(&self, url: &str) -> CacheLookupResult {
        let key = CacheKey::new(CacheResourceType::Segment, url, None);
        self.get(&key).await
    }

    /// Cache segment data
    pub async fn put_segment(&self, url: &str, data: Bytes) -> CacheResult<()> {
        let key = CacheKey::new(CacheResourceType::Segment, url, None);
        let metadata =
            CacheMetadata::new(data.len() as u64).with_expiration(self.config.segment_ttl);

        self.put(key, data, metadata).await
    }

    /// Get configuration reference
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// Perform maintenance tasks on the cache
    /// This should be called periodically to clean up expired entries
    pub async fn maintain(&self) -> CacheResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // For now, there's not much maintenance to do since Moka handles eviction
        // and FileCache doesn't have automatic cleanup
        // This is a placeholder for future implementation of file cache cleanup

        Ok(())
    }

    /// Start a background maintenance task
    pub fn start_maintenance_task(
        self: Arc<Self>,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                if let Err(e) = self.maintain().await {
                    tracing::warn!("Cache maintenance error: {}", e);
                }
            }
        })
    }
}
