//! # Cache System
//!
//! This module provides caching functionality for downloaded content,
//! helping to avoid redundant downloads and enabling features like
//! download resumption and content validation.

// Module declarations
mod manager;
pub mod providers;
mod types;
mod utils;

// Re-export primary types from our various modules
pub use manager::CacheManager;
pub use types::{
    CacheConfig, CacheKey, CacheLookupResult, CacheMetadata, CacheResourceType, CacheResult,
    CacheStatus,
};
pub use utils::extract_cache_headers;

pub use providers::{CacheProvider, FileCache, MemoryCache};
