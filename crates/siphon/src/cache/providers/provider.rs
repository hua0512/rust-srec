//! # Cache Provider
//!
//! This module defines the cache provider trait that all cache implementations must follow.

use async_trait::async_trait;
use bytes::Bytes;

use crate::cache::types::{CacheKey, CacheLookupResult, CacheMetadata, CacheResult};

/// A trait for cache providers that can store and retrieve cached data
#[async_trait]
pub trait CacheProvider: Send + Sync {
    /// Check if the cache contains an entry for the given key
    async fn contains(&self, key: &CacheKey) -> CacheResult<bool>;

    /// Get an entry from the cache
    async fn get(&self, key: &CacheKey) -> CacheLookupResult;

    /// Put an entry into the cache
    async fn put(&self, key: CacheKey, data: Bytes, metadata: CacheMetadata) -> CacheResult<()>;

    /// Remove an entry from the cache
    async fn remove(&self, key: &CacheKey) -> CacheResult<()>;

    /// Clear all entries from the cache
    async fn clear(&self) -> CacheResult<()>;

    /// Remove expired entries from the cache
    async fn sweep(&self) -> CacheResult<()>;
}
