//! # Cache Types
//!
//! This module defines common types used across the caching system.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Status of a cached resource
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// Resource found in cache and is valid
    Hit,
    /// Resource not found in cache
    Miss,
    /// Resource found but was validated against source
    Validated,
    /// Resource found but has expired
    Expired,
}

/// Types of resources that can be cached
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheResourceType {
    /// HTTP response headers
    Headers,
    /// Raw content bytes
    Content,
    /// HTTP responses
    Response,
    /// Media playlist
    Playlist,
    /// Media segment
    Segment,
    /// Decryption key
    Key,
}

/// Cache key for identifying resources
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Type of resource
    pub resource_type: CacheResourceType,
    /// URL of the resource
    pub url: String,
    /// Optional identifier for the resource
    pub identifier: Option<String>,
}

impl CacheKey {
    /// Create a new cache key
    pub fn new(
        resource_type: CacheResourceType,
        url: impl Into<String>,
        identifier: Option<String>,
    ) -> Self {
        Self {
            resource_type,
            url: url.into(),
            identifier,
        }
    }

    /// Convert to a filename-safe string
    pub fn to_filename(&self) -> String {
        use sha2::{Digest, Sha256};

        // Create a unique identifier for this cache key
        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}:{}", self.resource_type, self.url));
        if let Some(id) = &self.identifier {
            hasher.update(":");
            hasher.update(id);
        }

        let hash = hasher.finalize();
        format!("{hash:x}")
    }
}

/// Metadata for a cached resource
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// When the resource was cached
    pub cached_at: u64,
    /// When the resource expires
    pub expires_at: Option<u64>,
    /// ETag value if available
    pub etag: Option<String>,
    /// Last-Modified header value if available
    pub last_modified: Option<String>,
    /// Content type of the resource
    pub content_type: Option<String>,
    /// Size of the cached resource in bytes
    pub size: u64,
}

impl CacheMetadata {
    /// Create new metadata for a resource
    pub fn new(size: u64) -> Self {
        Self {
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            expires_at: None,
            etag: None,
            last_modified: None,
            content_type: None,
            size,
        }
    }

    /// Set the expiration time
    pub fn with_expiration(mut self, duration: Duration) -> Self {
        self.expires_at = Some(self.cached_at + duration.as_secs());
        self
    }

    /// Set the expiration time based on resource type
    pub fn with_expiration_by_type(
        mut self,
        resource_type: CacheResourceType,
        config: &CacheConfig,
    ) -> Self {
        let ttl = match resource_type {
            CacheResourceType::Playlist => config.playlist_ttl,
            CacheResourceType::Segment => config.segment_ttl,
            _ => config.default_ttl,
        };

        self.expires_at = Some(self.cached_at + ttl.as_secs());
        self
    }

    /// Set the ETag value
    pub fn with_etag(mut self, etag: impl Into<String>) -> Self {
        self.etag = Some(etag.into());
        self
    }

    /// Set the ETag value as an Option
    pub fn with_etag_option(mut self, etag: Option<String>) -> Self {
        self.etag = etag;
        self
    }

    /// Set the Last-Modified value
    pub fn with_last_modified(mut self, last_modified: impl Into<String>) -> Self {
        self.last_modified = Some(last_modified.into());
        self
    }

    /// Set the Last-Modified value as an Option
    pub fn with_last_modified_option(mut self, last_modified: Option<String>) -> Self {
        self.last_modified = last_modified;
        self
    }

    /// Set the content type
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the content type as an Option
    pub fn with_content_type_option(mut self, content_type: Option<String>) -> Self {
        self.content_type = content_type;
        self
    }

    /// Check if the resource has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            expires_at < now
        } else {
            false
        }
    }
}

/// Configuration for the cache system
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Whether caching is enabled
    pub enabled: bool,
    /// Path for disk cache storage
    pub disk_cache_path: Option<PathBuf>,
    /// Maximum size of disk cache in bytes
    pub max_disk_cache_size: u64,
    /// Maximum size of memory cache in bytes
    pub max_memory_cache_size: u64,
    /// Default TTL for cached content
    pub default_ttl: Duration,
    /// TTL for media segments
    pub segment_ttl: Duration,
    /// TTL for media playlists
    pub playlist_ttl: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disk_cache_path: None, // If None, we'll use system temp dir
            max_disk_cache_size: 500 * 1024 * 1024, // 500MB
            max_memory_cache_size: 30 * 1024 * 1024, // 30MB
            default_ttl: Duration::from_secs(3600), // 1 hour
            segment_ttl: Duration::from_secs(2 * 60), // 2 minutes
            playlist_ttl: Duration::from_secs(60), // 1 minute
        }
    }
}

/// Result of a cache operation
pub type CacheResult<T> = std::result::Result<T, std::io::Error>;

/// A type representing the result of a cache lookup operation
pub type CacheLookupResult = CacheResult<Option<(Bytes, CacheMetadata, CacheStatus)>>;
