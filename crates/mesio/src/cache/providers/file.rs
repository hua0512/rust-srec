//! # File Cache
//!
//! This module implements a file-based persistent cache provider.

use std::path::PathBuf;

use bytes::Bytes;
use tokio::fs;
use tokio::io;
use tracing::{debug, warn};

use crate::cache::types::{CacheKey, CacheLookupResult, CacheMetadata, CacheResult, CacheStatus};

use super::CacheProvider;

#[derive(Debug, Clone)]
pub struct FileCache {
    cache_dir: PathBuf,
    initialized: std::sync::Arc<std::sync::atomic::AtomicBool>,
    enabled: bool,
}

impl FileCache {
    /// Create a new file cache with the specified directory
    pub fn new(cache_dir: PathBuf, enabled: bool) -> Self {
        Self {
            cache_dir,
            initialized: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            enabled,
        }
    }

    /// Initialize the cache directories
    pub(crate) async fn ensure_initialized(&self) -> io::Result<()> {
        use std::sync::atomic::Ordering;

        // Fast path - already initialized
        if self.initialized.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Not enabled, nothing to initialize
        if !self.enabled {
            return Ok(());
        }

        // Use compare_exchange to ensure only one thread initializes
        if self
            .initialized
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            // We won the race, do initialization
            fs::create_dir_all(&self.cache_dir).await?;

            // Create subdirectories for different resource types
            for res_type in &[
                crate::cache::types::CacheResourceType::Headers,
                crate::cache::types::CacheResourceType::Content,
                crate::cache::types::CacheResourceType::Response,
                crate::cache::types::CacheResourceType::Playlist,
                crate::cache::types::CacheResourceType::Segment,
                crate::cache::types::CacheResourceType::Key,
            ] {
                fs::create_dir_all(self.cache_dir.join(format!("{res_type:?}"))).await?;
            }

            // Mark as fully initialized with release ordering
            self.initialized.store(true, Ordering::Release);
        } else {
            // Another thread is initializing, wait for it to complete
            while !self.initialized.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        }

        Ok(())
    }

    /// Get the path for a cached resource
    fn get_cache_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir
            .join(format!("{:?}", key.resource_type))
            .join(key.to_filename())
    }

    /// Get the metadata path for a cached resource
    fn get_metadata_path(&self, key: &CacheKey) -> PathBuf {
        let mut path = self.get_cache_path(key);
        path.set_extension("meta");
        path
    }
}

#[async_trait::async_trait]
impl CacheProvider for FileCache {
    async fn contains(&self, key: &CacheKey) -> CacheResult<bool> {
        if !self.enabled {
            return Ok(false);
        }

        self.ensure_initialized().await?;

        let data_path = self.get_cache_path(key);
        let meta_path = self.get_metadata_path(key);

        // Use tokio::fs::try_exists for async file existence check
        // This is more efficient than checking both files separately
        let data_exists = fs::try_exists(&data_path).await?;
        let meta_exists = fs::try_exists(&meta_path).await?;

        Ok(data_exists && meta_exists)
    }

    async fn get(&self, key: &CacheKey) -> CacheLookupResult {
        if !self.enabled {
            return Ok(None);
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        let data_path = self.get_cache_path(key);
        let meta_path = self.get_metadata_path(key);

        // Check if both data and metadata exist
        let data_exists = fs::try_exists(&data_path).await?;
        let meta_exists = fs::try_exists(&meta_path).await?;

        if !data_exists || !meta_exists {
            return Ok(None);
        }

        // Read metadata
        let metadata_bytes = match fs::read(&meta_path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(path = ?meta_path, error = %e, "Failed to read cache metadata file");
                return Ok(None);
            }
        };

        let metadata: CacheMetadata = match serde_json::from_slice(&metadata_bytes) {
            Ok(m) => m,
            Err(e) => {
                warn!(path = ?meta_path, error = %e, "Failed to parse cache metadata");

                // Delete invalid cache entry as a background task
                // We use spawn to avoid blocking the current task
                let data_path_clone = data_path.clone();
                let meta_path_clone = meta_path.clone();
                tokio::spawn(async move {
                    let _ = fs::remove_file(&data_path_clone).await;
                    let _ = fs::remove_file(&meta_path_clone).await;
                });

                return Ok(None);
            }
        };

        // Check if expired
        let status = if metadata.is_expired() {
            CacheStatus::Expired
        } else {
            CacheStatus::Hit
        };

        // Read data
        let data = match fs::read(&data_path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(path = ?data_path, error = %e, "Failed to read cache data file");
                return Ok(None);
            }
        };

        // For expired entries, we can still return the data, but remove it in the background
        if status == CacheStatus::Expired {
            let data_path_clone = data_path.clone();
            let meta_path_clone = meta_path.clone();
            tokio::spawn(async move {
                let _ = fs::remove_file(&data_path_clone).await;
                let _ = fs::remove_file(&meta_path_clone).await;
            });
        }
        let bytes = Bytes::from(data);

        Ok(Some((bytes, metadata, status)))
    }

    async fn put(&self, key: CacheKey, data: Bytes, metadata: CacheMetadata) -> CacheResult<()> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        let data_path = self.get_cache_path(&key);
        let meta_path = self.get_metadata_path(&key);

        // Create parent directory if it doesn't exist
        if let Some(parent) = data_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Serialize metadata to JSON
        let metadata_json = match serde_json::to_vec(&metadata) {
            Ok(json) => json,
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to serialize metadata: {e}"),
                ));
            }
        };

        // Write data and metadata atomically if possible
        // First write to temporary files then rename
        let temp_data_path = data_path.with_extension("tmp");
        let temp_meta_path = meta_path.with_extension("tmp");

        // Write data file
        match fs::write(&temp_data_path, &data).await {
            Ok(_) => {}
            Err(e) => {
                warn!(path = ?temp_data_path, error = %e, "Failed to write cache data file");
                return Err(e);
            }
        }

        // Write metadata file
        match fs::write(&temp_meta_path, &metadata_json).await {
            Ok(_) => {}
            Err(e) => {
                warn!(path = ?temp_meta_path, error = %e, "Failed to write cache metadata file");
                // Clean up data file
                let _ = fs::remove_file(&temp_data_path).await;
                return Err(e);
            }
        }

        // Rename temp files to final filenames
        // This makes the operation more atomic and reduces the chance of incomplete writes
        if let Err(e) = fs::rename(&temp_data_path, &data_path).await {
            warn!(
                from = ?temp_data_path,
                to = ?data_path,
                error = %e,
                "Failed to rename temporary data file"
            );
            // Clean up
            let _ = fs::remove_file(&temp_data_path).await;
            let _ = fs::remove_file(&temp_meta_path).await;
            return Err(e);
        }

        if let Err(e) = fs::rename(&temp_meta_path, &meta_path).await {
            warn!(
                from = ?temp_meta_path,
                to = ?meta_path,
                error = %e,
                "Failed to rename temporary metadata file"
            );
            // We successfully renamed the data file but not the metadata
            // This is an inconsistent state, so try to clean up
            let _ = fs::remove_file(&data_path).await;
            let _ = fs::remove_file(&temp_meta_path).await;
            return Err(e);
        }

        debug!(key = ?key, "Successfully cached entry to file");
        Ok(())
    }

    async fn remove(&self, key: &CacheKey) -> CacheResult<()> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        let data_path = self.get_cache_path(key);
        let meta_path = self.get_metadata_path(key);

        // Try to remove both files
        // We don't care if the files don't exist
        let data_result = fs::remove_file(&data_path).await;
        let meta_result = fs::remove_file(&meta_path).await;

        // If both operations error, return the data error
        // If only one errors, return that error
        match (data_result, meta_result) {
            (Err(e), _) if e.kind() != io::ErrorKind::NotFound => {
                warn!(path = ?data_path, error = %e, "Failed to remove cache data file");
                Err(e)
            }
            (_, Err(e)) if e.kind() != io::ErrorKind::NotFound => {
                warn!(path = ?meta_path, error = %e, "Failed to remove cache metadata file");
                Err(e)
            }
            _ => Ok(()),
        }
    }

    async fn clear(&self) -> CacheResult<()> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        // Remove everything from cache directory
        let mut entries = match fs::read_dir(&self.cache_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(dir = ?self.cache_dir, error = %e, "Failed to read cache directory");
                return Err(e);
            }
        };

        let mut entry_count = 0;

        // Process all entries in the cache directory
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                if let Err(e) = fs::remove_dir_all(&path).await {
                    warn!(path = ?path, error = %e, "Failed to remove cache subdirectory");
                } else {
                    entry_count += 1;
                }
            } else if let Err(e) = fs::remove_file(&path).await {
                warn!(path = ?path, error = %e, "Failed to remove cache file");
            } else {
                entry_count += 1;
            }
        }

        debug!(count = entry_count, "Cleared cache entries");

        // Reset initialized state and recreate subdirectories
        self.initialized
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.ensure_initialized().await?;

        Ok(())
    }

    async fn sweep(&self) -> CacheResult<()> {
        // This is a no-op for file cache
        // File cache doesn't need to sweep as it uses file system for expiration
        Ok(())
    }
}
