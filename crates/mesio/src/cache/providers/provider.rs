//! # Cache Provider
//!
//! This module defines the cache provider trait that all cache implementations must follow.

use std::future::Future;

use bytes::Bytes;

use crate::cache::types::{CacheKey, CacheLookupResult, CacheMetadata, CacheResult};

/// A trait for cache providers that can store and retrieve cached data
pub trait CacheProvider: Send + Sync {
    /// Check if the cache contains an entry for the given key
    fn contains(&self, key: &CacheKey) -> impl Future<Output = CacheResult<bool>> + Send;

    /// Get an entry from the cache
    fn get(&self, key: &CacheKey) -> impl Future<Output = CacheLookupResult> + Send;

    /// Put an entry into the cache
    fn put(
        &self,
        key: CacheKey,
        data: Bytes,
        metadata: CacheMetadata,
    ) -> impl Future<Output = CacheResult<()>> + Send;

    /// Remove an entry from the cache
    fn remove(&self, key: &CacheKey) -> impl Future<Output = CacheResult<()>> + Send;

    /// Clear all entries from the cache
    fn clear(&self) -> impl Future<Output = CacheResult<()>> + Send;

    /// Remove expired entries from the cache
    fn sweep(&self) -> impl Future<Output = CacheResult<()>> + Send;
}
