//! Bounded executable capability probes, separate from per-attempt verification.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use parking_lot::Mutex;
use tokio::sync::{OnceCell, Semaphore};
use tokio::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);
const CAPACITY: usize = 16;

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    invocation: String,
    launch_path: PathBuf,
    executable: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
    python_path: Option<OsString>,
    python_home: Option<OsString>,
}

struct Entry {
    result: Arc<OnceCell<bool>>,
    expires: Instant,
}

struct CapabilityCache {
    entries: Mutex<HashMap<Key, Entry>>,
    probes: Semaphore,
    capacity: usize,
}

impl CapabilityCache {
    fn new(capacity: usize, concurrency: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            probes: Semaphore::new(concurrency),
            capacity,
        }
    }

    async fn check<F, Fut>(&self, key: Key, probe: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let result = {
            let mut entries = self.entries.lock();
            let now = Instant::now();
            entries.retain(|_, entry| entry.expires > now || Arc::strong_count(&entry.result) > 1);
            if !entries.contains_key(&key) && entries.len() >= self.capacity {
                let oldest = entries
                    .iter()
                    .filter(|(_, entry)| Arc::strong_count(&entry.result) == 1)
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    entries.remove(&oldest);
                } else {
                    return false;
                } // preserve ordinary recording while admission is full
            }
            entries
                .entry(key)
                .or_insert_with(|| Entry {
                    result: Arc::new(OnceCell::new()),
                    expires: now + TTL,
                })
                .result
                .clone()
        };
        *result
            .get_or_init(|| async {
                let Ok(_permit) = self.probes.acquire().await else {
                    return false;
                };
                probe().await
            })
            .await
    }
}

async fn identity(binary: &str) -> Option<Key> {
    let path = Path::new(binary);
    let mut candidates = Vec::new();
    if path.is_absolute() || path.components().count() > 1 {
        candidates.push(path.to_path_buf());
    } else {
        #[cfg(windows)]
        {
            if let Ok(executable) = std::env::current_exe()
                && let Some(parent) = executable.parent()
            {
                candidates.push(parent.join(path));
            }
            if let Ok(current) = std::env::current_dir() {
                candidates.push(current.join(path));
            }
            if let Some(windows) = std::env::var_os("SYSTEMROOT") {
                candidates.push(Path::new(&windows).join("System32").join(path));
                candidates.push(Path::new(&windows).join(path));
            }
        }
        if let Some(search) = std::env::var_os("PATH") {
            candidates.extend(
                std::env::split_paths(&search)
                    .take(256)
                    .map(|directory| directory.join(path)),
            );
        }
    }
    for candidate in candidates {
        #[cfg(not(windows))]
        let names = vec![candidate];
        #[cfg(windows)]
        let names = if candidate.extension().is_none() {
            vec![candidate.with_extension("exe"), candidate]
        } else {
            vec![candidate]
        };
        for name in names {
            let Ok(metadata) = tokio::fs::metadata(&name).await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            let Ok(executable) = tokio::fs::canonicalize(&name).await else {
                continue;
            };
            return Some(Key {
                invocation: binary.to_owned(),
                launch_path: name,
                executable,
                length: metadata.len(),
                modified: metadata.modified().ok(),
                python_path: std::env::var_os("PYTHONPATH"),
                python_home: std::env::var_os("PYTHONHOME"),
            });
        }
    }
    None
}

pub(super) async fn capability(binary: &str) -> bool {
    static CACHE: OnceLock<CapabilityCache> = OnceLock::new();
    let Ok(Some(key)) = tokio::time::timeout(Duration::from_millis(500), identity(binary)).await
    else {
        return false;
    };
    CACHE
        .get_or_init(|| CapabilityCache::new(CAPACITY, 4))
        .check(key, || super::Companion::probe(binary))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn key(length: u64) -> Key {
        Key {
            invocation: "fixture".into(),
            launch_path: "fixture".into(),
            executable: PathBuf::from("fixture"),
            length,
            modified: None,
            python_path: None,
            python_home: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn probes_are_coalesced_cached_and_invalidated_by_identity_or_expiry() {
        let cache = CapabilityCache::new(4, 1);
        let calls = AtomicUsize::new(0);
        let release = Notify::new();
        let mut first = Box::pin(cache.check(key(1), || async {
            calls.fetch_add(1, Ordering::Relaxed);
            release.notified().await;
            true
        }));
        let mut second = Box::pin(cache.check(key(1), || async { panic!("duplicate probe") }));
        assert!(futures::poll!(&mut first).is_pending());
        assert!(futures::poll!(&mut second).is_pending());
        drop(second); // a waiter cannot cancel the admitted initializer
        release.notify_one();
        assert!(first.await);
        assert!(
            cache
                .check(key(1), || async { panic!("cached probe") })
                .await
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(
            !cache
                .check(key(2), || async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    false
                })
                .await
        );
        tokio::time::advance(TTL + Duration::from_secs(1)).await;
        assert!(
            cache
                .check(key(1), || async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    true
                })
                .await
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn cancelling_the_initializer_releases_admission_and_allows_a_later_probe() {
        let cache = CapabilityCache::new(2, 1);
        let calls = AtomicUsize::new(0);
        let mut cancelled = Box::pin(cache.check(key(1), || async {
            calls.fetch_add(1, Ordering::Relaxed);
            std::future::pending().await
        }));
        assert!(futures::poll!(&mut cancelled).is_pending());
        drop(cancelled);
        assert!(
            cache
                .check(key(1), || async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    true
                })
                .await
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn distinct_inflight_keys_and_probe_execution_are_bounded() {
        let cache = CapabilityCache::new(2, 1);
        let mut first = Box::pin(cache.check(key(1), std::future::pending));
        let mut second = Box::pin(cache.check(key(2), || async { panic!("probe limit") }));
        assert!(futures::poll!(&mut first).is_pending());
        assert!(futures::poll!(&mut second).is_pending());
        assert!(
            !cache
                .check(key(3), || async { panic!("entry limit") })
                .await
        );
        drop(first);
        drop(second);
        assert!(cache.check(key(3), || async { true }).await);
    }

    #[tokio::test]
    async fn changing_the_configured_executable_changes_its_cache_identity() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("streamlink.exe");
        tokio::fs::write(&executable, b"first executable")
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
                .await
                .unwrap();
        }
        let first = identity(executable.to_str().unwrap()).await.unwrap();
        tokio::fs::write(&executable, b"replacement executable contents")
            .await
            .unwrap();
        let second = identity(executable.to_str().unwrap()).await.unwrap();
        assert!(first != second);
        assert!(identity(directory.path().to_str().unwrap()).await.is_none());
    }

    #[tokio::test]
    async fn invocation_aliases_do_not_share_an_argv_dependent_capability() {
        let cache = CapabilityCache::new(4, 1);
        let first = key(1);
        let mut alias = first.clone();
        alias.invocation = "reject-plugin-option".into();
        alias.launch_path = "reject-plugin-option".into();
        assert!(cache.check(first, || async { true }).await);
        assert!(!cache.check(alias, || async { false }).await);
    }
}
