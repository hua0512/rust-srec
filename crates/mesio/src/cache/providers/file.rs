//! # File Cache
//!
//! This module implements a file-based persistent cache provider.
//! Each `.entry` file contains a versioned header, JSON metadata, and payload,
//! published together by one rename. Legacy data/`.meta` pairs are cache misses
//! (they cannot provide a consistent snapshot), but remain eligible for sweep.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use tokio::{
    fs,
    io::{self, AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
use tracing::{debug, warn};

use crate::cache::types::{CacheKey, CacheLookupResult, CacheMetadata, CacheResult, CacheStatus};

use super::CacheProvider;

/// Distinguishes temp files of concurrent puts within this process.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

const ENTRY_MAGIC: &[u8; 8] = b"MESIOC01";
// Metadata contains only timestamps, size, and a few HTTP header values. Bound
// its length before allocating so a corrupt header cannot request gigabytes.
const MAX_METADATA_BYTES: usize = 1024 * 1024;
/// Temp files untouched for this long belong to a writer that died before
/// publishing. A live write keeps modifying its file, and cache payloads are
/// written in one pass, so this is far beyond any in-progress write.
const STALE_TEMP_FILE_AGE: Duration = Duration::from_secs(60 * 60);

/// The pid and counter separate concurrent writes, including across cache
/// instances/processes; the `tmp` extension lets sweep skip unfinished files.
fn temp_path_for(final_path: &Path) -> PathBuf {
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    let n = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    final_path.with_file_name(name)
}

#[derive(Debug, Clone)]
pub struct FileCache {
    cache_dir: PathBuf,
    initialized: Arc<AtomicBool>,
    init_lock: Arc<Mutex<()>>,
    enabled: bool,
    /// Maximum disk cache size in bytes (0 = unlimited)
    max_size: u64,
}

impl FileCache {
    async fn remove_file_best_effort(path: &Path, context: &'static str) {
        match fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => warn!(path = ?path, %error, context, "Cache cleanup failed"),
        }
    }

    /// Create a new file cache with the specified directory and size limit
    pub fn new(cache_dir: PathBuf, enabled: bool, max_size: u64) -> Self {
        Self {
            cache_dir,
            initialized: Arc::new(AtomicBool::new(false)),
            init_lock: Arc::new(Mutex::new(())),
            enabled,
            max_size,
        }
    }

    /// Initialize the cache directories
    pub(crate) async fn ensure_initialized(&self) -> io::Result<()> {
        // Fast path - already initialized
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        // Not enabled, nothing to initialize
        if !self.enabled {
            return Ok(());
        }

        let _guard = self.init_lock.lock().await;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        fs::create_dir_all(&self.cache_dir).await?;

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

        // Publish readiness only after every directory was created successfully.
        self.initialized.store(true, Ordering::Release);

        Ok(())
    }

    /// Get the path for a cached resource
    fn get_cache_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir
            .join(format!("{:?}", key.resource_type))
            .join(key.to_filename())
            .with_extension("entry")
    }

    /// Leave the same open file positioned at its payload. Reopening by path
    /// after reading metadata could observe a concurrent replacement.
    async fn read_metadata(file: &mut fs::File) -> io::Result<CacheMetadata> {
        let mut magic = [0; 8];
        file.read_exact(&mut magic).await?;
        if &magic != ENTRY_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid cache format",
            ));
        }
        let metadata_len = file.read_u32().await? as usize;
        if metadata_len > MAX_METADATA_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cache metadata too large",
            ));
        }
        let mut json = vec![0; metadata_len];
        file.read_exact(&mut json).await?;
        serde_json::from_slice(&json)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Remove `path` only if it is still the file sweep inspected. A put that
    /// published a fresh generation in between replaced the file, so its
    /// identity differs and it is kept. The check narrows, but cannot close,
    /// the window before removal; losing an entry there only costs a miss.
    async fn remove_if_unchanged(path: &Path, observed: &std::fs::Metadata, context: &'static str) {
        let Ok(current) = fs::metadata(path).await else {
            return;
        };
        if same_file_generation(&current, observed) {
            Self::remove_file_best_effort(path, context).await;
        }
    }

    async fn write_entry(path: &Path, data: &Bytes, metadata_json: &[u8]) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        file.write_all(ENTRY_MAGIC).await?;
        // put() bounds this to MAX_METADATA_BYTES before opening the temp file.
        file.write_u32(metadata_json.len() as u32).await?;
        file.write_all(metadata_json).await?;
        file.write_all(data).await?;
        // Tokio may still have a blocking write in flight when write_all
        // returns. Finish it before closing and publishing the file.
        file.flush().await
    }
}

#[cfg(unix)]
fn same_file_generation(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file_generation(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

impl CacheProvider for FileCache {
    async fn contains(&self, key: &CacheKey) -> CacheResult<bool> {
        if !self.enabled {
            return Ok(false);
        }

        self.ensure_initialized().await?;

        fs::try_exists(self.get_cache_path(key)).await
    }

    async fn get(&self, key: &CacheKey) -> CacheLookupResult {
        if !self.enabled {
            return Ok(None);
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        let path = self.get_cache_path(key);
        let mut file = match fs::File::open(&path).await {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                warn!(?path, %error, "Failed to open cache entry");
                return Ok(None);
            }
        };
        let metadata = match Self::read_metadata(&mut file).await {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(?path, %error, "Failed to read cache metadata");
                return Ok(None);
            }
        };

        // Check if expired
        let status = if metadata.is_expired() {
            CacheStatus::Expired
        } else {
            CacheStatus::Hit
        };

        let mut data = Vec::new();
        if let Err(error) = file.read_to_end(&mut data).await {
            warn!(?path, %error, "Failed to read cache payload");
            return Ok(None);
        }
        if data.len() as u64 != metadata.size {
            warn!(?path, "Cache payload length does not match metadata");
            return Ok(None);
        }

        // Cleanup belongs to sweep: deleting this path after reading an expired
        // or invalid generation could remove a concurrently published fresh one.

        Ok(Some((Bytes::from(data), metadata, status)))
    }

    async fn put(&self, key: CacheKey, data: Bytes, metadata: CacheMetadata) -> CacheResult<()> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure cache is initialized
        self.ensure_initialized().await?;

        let data_path = self.get_cache_path(&key);

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

        // get() rejects a payload whose length differs from metadata.size, so
        // such an entry could never be read back.
        if metadata.size != data.len() as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Cache metadata size {} does not match payload length {}",
                    metadata.size,
                    data.len()
                ),
            ));
        }

        if metadata_json.len() > MAX_METADATA_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cache metadata too large",
            ));
        }

        let temp_data_path = loop {
            let path = temp_path_for(&data_path);
            match Self::write_entry(&path, &data, &metadata_json).await {
                Ok(()) => break path,
                // A previous process with the same pid may have left a temp file.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    Self::remove_file_best_effort(&path, "temporary cache entry").await;
                    return Err(error);
                }
            }
        };

        // One rename publishes both fields. Readers open this file once, so
        // even independent processes see one complete generation of the entry.
        if let Err(e) = fs::rename(&temp_data_path, &data_path).await {
            warn!(
                from = ?temp_data_path,
                to = ?data_path,
                error = %e,
                "Failed to rename temporary data file"
            );
            // Clean up
            Self::remove_file_best_effort(&temp_data_path, "temporary cache data").await;
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
        match fs::remove_file(&data_path).await {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                warn!(path = ?data_path, error = %e, "Failed to remove cache data file");
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
        if !self.enabled {
            return Ok(());
        }

        self.ensure_initialized().await?;
        let now = SystemTime::now();

        // Collect all cached entries with their metadata
        // Legacy entries retain their sidecar path only for eviction.
        let mut entries: Vec<(PathBuf, Option<PathBuf>, u64, u64)> = Vec::new();
        let mut total_size: u64 = 0;

        // Scan all subdirectories for cache entries
        let mut dir_entries = match fs::read_dir(&self.cache_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(dir = ?self.cache_dir, error = %e, "Failed to read cache directory for sweep");
                return Ok(());
            }
        };

        while let Ok(Some(subdir_entry)) = dir_entries.next_entry().await {
            let subdir_path = subdir_entry.path();
            if !subdir_path.is_dir() {
                continue;
            }

            let mut subdir_entries = match fs::read_dir(&subdir_path).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = subdir_entries.next_entry().await {
                let path = entry.path();

                // Skip metadata files (we'll handle them with their data files)
                if path.extension().is_some_and(|ext| ext == "meta") {
                    continue;
                }

                // Unfinished writes are skipped; ones abandoned by a writer
                // that died before publishing are reclaimed.
                if path.extension().is_some_and(|ext| ext == "tmp") {
                    if let Ok(metadata) = fs::metadata(&path).await
                        && let Ok(modified) = metadata.modified()
                        && now
                            .duration_since(modified)
                            .is_ok_and(|age| age >= STALE_TEMP_FILE_AGE)
                    {
                        Self::remove_file_best_effort(&path, "abandoned temporary cache entry")
                            .await;
                    }
                    continue;
                }

                if path.extension().is_some_and(|ext| ext == "entry") {
                    let mut file = match fs::File::open(&path).await {
                        Ok(file) => file,
                        Err(_) => continue,
                    };
                    // Size and identity come from the opened handle, so they
                    // describe the same generation as the metadata read below.
                    let observed = match file.metadata().await {
                        Ok(m) if m.is_file() => m,
                        _ => continue,
                    };
                    let file_size = observed.len();
                    // Invalid entries are oldest for eviction, so corrupt
                    // files cannot escape the disk-size bound.
                    let cached_at = match Self::read_metadata(&mut file).await {
                        Ok(metadata) if metadata.is_expired() => {
                            // Expired entries are never served as hits, so
                            // reclaim them whether or not a size limit is set.
                            drop(file);
                            Self::remove_if_unchanged(&path, &observed, "expired cache entry")
                                .await;
                            continue;
                        }
                        Ok(metadata) => metadata.cached_at,
                        Err(_) => 0,
                    };
                    total_size += file_size;
                    entries.push((path, None, file_size, cached_at));
                } else if path.extension().is_none() {
                    let file_size = match fs::metadata(&path).await {
                        Ok(m) if m.is_file() => m.len(),
                        _ => continue,
                    };
                    let meta_path = path.with_extension("meta");
                    let meta_size = match fs::metadata(&meta_path).await {
                        Ok(metadata) => metadata.len(),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
                        Err(_) => continue,
                    };
                    let entry_size = file_size + meta_size;
                    total_size += entry_size;
                    // Legacy pairs are no longer read; evict them first.
                    entries.push((path, Some(meta_path), entry_size, 0));
                }
            }
        }

        // Check if we're over the limit (0 = unlimited)
        if self.max_size == 0 || total_size <= self.max_size {
            debug!(
                total_size = total_size,
                max_size = self.max_size,
                "Disk cache within limits, no eviction needed"
            );
            return Ok(());
        }

        // Sort by cached_at (oldest first) for LRU eviction
        entries.sort_by_key(|(_, _, _, cached_at)| *cached_at);

        // Target 80% of max_size to avoid constant eviction cycles
        let target_size = (self.max_size as f64 * 0.8) as u64;
        let mut evicted_count = 0;
        let mut evicted_size: u64 = 0;

        for (data_path, meta_path, entry_size, _) in entries {
            if total_size <= target_size {
                break;
            }

            // Removing a current-format entry cannot strand a metadata half.
            if let Err(e) = fs::remove_file(&data_path).await
                && e.kind() != io::ErrorKind::NotFound
            {
                warn!(path = ?data_path, error = %e, "Failed to remove cache data file during sweep");
            }

            if let Some(meta_path) = meta_path
                && let Err(e) = fs::remove_file(&meta_path).await
                && e.kind() != io::ErrorKind::NotFound
            {
                warn!(path = ?meta_path, error = %e, "Failed to remove cache metadata file during sweep");
            }

            total_size = total_size.saturating_sub(entry_size);
            evicted_size += entry_size;
            evicted_count += 1;
        }

        if evicted_count > 0 {
            debug!(
                evicted_count = evicted_count,
                evicted_size = evicted_size,
                remaining_size = total_size,
                max_size = self.max_size,
                "Evicted old cache entries to enforce disk limit"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::cache::types::CacheResourceType;

    use super::*;

    #[tokio::test]
    async fn initialization_failure_can_be_retried() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        fs::write(&cache_dir, b"not a directory").await.unwrap();
        let cache = FileCache::new(cache_dir.clone(), true, 1024);

        assert!(cache.ensure_initialized().await.is_err());
        assert!(!cache.initialized.load(Ordering::Acquire));

        fs::remove_file(&cache_dir).await.unwrap();
        cache.ensure_initialized().await.unwrap();

        assert!(cache.initialized.load(Ordering::Acquire));
        assert!(fs::try_exists(cache_dir.join("Content")).await.unwrap());
    }

    #[tokio::test]
    async fn put_then_get_round_trips_data_and_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().join("cache"), true, 0);
        let key = CacheKey::new(
            crate::cache::types::CacheResourceType::Content,
            "https://example.com/seg.ts".to_string(),
            None,
        );
        let data = Bytes::from_static(b"segment payload");

        cache
            .put(
                key.clone(),
                data.clone(),
                CacheMetadata::new(data.len() as u64),
            )
            .await
            .unwrap();

        let (cached, metadata, _) = cache.get(&key).await.unwrap().expect("entry cached");
        assert_eq!(cached, data);
        assert_eq!(metadata.size, data.len() as u64);

        let mut leftover_tmp = 0;
        let mut dir = fs::read_dir(cache.get_cache_path(&key).parent().unwrap())
            .await
            .unwrap();
        while let Some(entry) = dir.next_entry().await.unwrap() {
            if entry.path().extension().is_some_and(|ext| ext == "tmp") {
                leftover_tmp += 1;
            }
        }
        assert_eq!(leftover_tmp, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn independent_caches_publish_consistent_generations_under_contention() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key = CacheKey::new(
            CacheResourceType::Content,
            "https://example.com/shared",
            None,
        );
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = tokio::task::JoinSet::new();
        for writer in 1..=8_u8 {
            // Separate instances must work without a shared in-process lock.
            let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
            let key = key.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                for round in 0..16 {
                    let data = Bytes::from(vec![writer; 512 + writer as usize * 32 + round]);
                    let metadata = CacheMetadata::new(data.len() as u64)
                        .with_etag(format!("{writer}:{}", data.len()));
                    barrier.wait().await;
                    cache.put(key.clone(), data, metadata).await.unwrap();
                    let (data, metadata, status) =
                        cache.get(&key).await.unwrap().expect("complete entry");
                    assert_eq!(status, CacheStatus::Hit);
                    assert_eq!(data.len() as u64, metadata.size);
                    assert!(data.iter().all(|byte| *byte == data[0]));
                    assert_eq!(metadata.etag, Some(format!("{}:{}", data[0], data.len())));
                }
            });
        }
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("concurrent cache operations finish");
    }

    #[tokio::test]
    async fn persisted_entries_support_expiration_replacement_remove_and_clear() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
        let key = CacheKey::new(CacheResourceType::Content, "https://example.com/a", None);
        cache
            .put(
                key.clone(),
                Bytes::from_static(b"expired"),
                CacheMetadata::new(7).with_expiration(Duration::ZERO),
            )
            .await
            .unwrap();

        let reopened = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
        assert!(reopened.contains(&key).await.unwrap());
        let (_, _, status) = reopened.get(&key).await.unwrap().unwrap();
        assert_eq!(status, CacheStatus::Expired);
        reopened
            .put(
                key.clone(),
                Bytes::from_static(b"fresh"),
                CacheMetadata::new(5).with_etag("new"),
            )
            .await
            .unwrap();
        let (data, metadata, status) = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(data, b"fresh"[..]);
        assert_eq!(metadata.etag.as_deref(), Some("new"));
        assert_eq!(status, CacheStatus::Hit);
        cache.remove(&key).await.unwrap();
        cache.remove(&key).await.unwrap();
        assert!(!reopened.contains(&key).await.unwrap());
        assert!(reopened.get(&key).await.unwrap().is_none());

        cache
            .put(
                key.clone(),
                Bytes::from_static(b"again"),
                CacheMetadata::new(5),
            )
            .await
            .unwrap();
        cache.clear().await.unwrap();
        assert!(reopened.get(&key).await.unwrap().is_none());
        reopened
            .put(
                key.clone(),
                Bytes::from_static(b"after clear"),
                CacheMetadata::new(11),
            )
            .await
            .unwrap();
        assert_eq!(
            cache.get(&key).await.unwrap().unwrap().0,
            b"after clear"[..]
        );
    }

    #[tokio::test]
    async fn malformed_entries_are_misses_and_remain_evictable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 1);
        let key = CacheKey::new(CacheResourceType::Content, "https://example.com/a", None);
        cache
            .put(
                key.clone(),
                Bytes::from_static(b"payload"),
                CacheMetadata::new(7),
            )
            .await
            .unwrap();
        let path = cache.get_cache_path(&key);
        let valid = fs::read(&path).await.unwrap();
        let mut oversized_header = ENTRY_MAGIC.to_vec();
        oversized_header.extend_from_slice(&u32::MAX.to_be_bytes());
        for corrupt in [
            b"bad cache format".to_vec(),
            oversized_header,
            valid[..valid.len() - 1].to_vec(),
        ] {
            fs::write(&path, corrupt).await.unwrap();
            assert!(cache.get(&key).await.unwrap().is_none());
            cache.sweep().await.unwrap();
            assert!(!cache.contains(&key).await.unwrap());
        }
    }

    #[tokio::test]
    async fn sweep_reclaims_legacy_pairs_without_reading_them_or_unfinished_writes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 4096);
        cache.ensure_initialized().await.unwrap();
        let old_key = CacheKey::new(CacheResourceType::Content, "https://example.com/old", None);
        let old_path = temp_dir.path().join("Content").join(old_key.to_filename());
        let old_meta_path = old_path.with_extension("meta");
        fs::write(&old_path, vec![0; 8192]).await.unwrap();
        fs::write(
            &old_meta_path,
            serde_json::to_vec(&CacheMetadata::new(8192)).unwrap(),
        )
        .await
        .unwrap();
        assert!(cache.get(&old_key).await.unwrap().is_none());
        assert!(!cache.contains(&old_key).await.unwrap());

        let key = CacheKey::new(
            CacheResourceType::Content,
            "https://example.com/current",
            None,
        );
        cache
            .put(
                key.clone(),
                Bytes::from_static(b"fresh"),
                CacheMetadata::new(5),
            )
            .await
            .unwrap();
        let unfinished = temp_path_for(&cache.get_cache_path(&key));
        fs::write(&unfinished, vec![0; 8192]).await.unwrap();
        cache.sweep().await.unwrap();

        assert!(!fs::try_exists(&old_path).await.unwrap());
        assert!(!fs::try_exists(&old_meta_path).await.unwrap());
        assert!(fs::try_exists(&unfinished).await.unwrap());
        assert_eq!(cache.get(&key).await.unwrap().unwrap().0, b"fresh"[..]);
    }

    #[tokio::test]
    async fn put_rejects_metadata_size_that_does_not_match_payload() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
        let key = CacheKey::new(CacheResourceType::Content, "https://example.com/a", None);

        let error = cache
            .put(
                key.clone(),
                Bytes::from_static(b"payload"),
                CacheMetadata::new(3),
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!cache.contains(&key).await.unwrap());
    }

    #[tokio::test]
    async fn sweep_reclaims_expired_entries_without_a_size_limit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
        let expired = CacheKey::new(CacheResourceType::Content, "https://example.com/old", None);
        let fresh = CacheKey::new(CacheResourceType::Content, "https://example.com/new", None);
        cache
            .put(
                expired.clone(),
                Bytes::from_static(b"old"),
                CacheMetadata::new(3).with_expiration(Duration::ZERO),
            )
            .await
            .unwrap();
        cache
            .put(
                fresh.clone(),
                Bytes::from_static(b"new"),
                CacheMetadata::new(3),
            )
            .await
            .unwrap();

        cache.sweep().await.unwrap();

        assert!(!cache.contains(&expired).await.unwrap());
        assert_eq!(cache.get(&fresh).await.unwrap().unwrap().0, b"new"[..]);
    }

    #[tokio::test]
    async fn sweep_reclaims_abandoned_temp_files_but_keeps_recent_ones() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(temp_dir.path().to_path_buf(), true, 0);
        cache.ensure_initialized().await.unwrap();
        let key = CacheKey::new(CacheResourceType::Content, "https://example.com/a", None);
        let abandoned = temp_path_for(&cache.get_cache_path(&key));
        let recent = temp_path_for(&cache.get_cache_path(&key));
        fs::write(&abandoned, b"partial").await.unwrap();
        fs::write(&recent, b"partial").await.unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&abandoned)
            .unwrap()
            .set_modified(SystemTime::now() - STALE_TEMP_FILE_AGE - Duration::from_secs(60))
            .unwrap();

        cache.sweep().await.unwrap();

        assert!(!fs::try_exists(&abandoned).await.unwrap());
        assert!(fs::try_exists(&recent).await.unwrap());
    }
}
