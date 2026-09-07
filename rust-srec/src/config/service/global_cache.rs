use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::Mutex;

use crate::database::models::GlobalConfigDbModel;
use crate::{Error, Result};

// Bounds staleness for out-of-band database edits. Service writes and imports invalidate
// immediately; an expired entry is never used to hide a failed refresh.
const GLOBAL_CONFIG_TTL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct State {
    generation: u64,
    value: Option<(Instant, Arc<GlobalConfigDbModel>)>,
}

#[derive(Default)]
pub(super) struct GlobalConfigCache {
    state: RwLock<State>,
    fill: Mutex<()>,
}

impl GlobalConfigCache {
    fn current(&self) -> Option<Arc<GlobalConfigDbModel>> {
        self.state
            .read()
            .value
            .as_ref()
            .filter(|(loaded, _)| loaded.elapsed() < GLOBAL_CONFIG_TTL)
            .map(|(_, value)| value.clone())
    }

    pub(super) fn invalidate(&self) {
        let mut state = self.state.write();
        state.generation = state.generation.wrapping_add(1);
        state.value = None;
    }

    pub(super) async fn get_or_load<F, Fut>(&self, mut load: F) -> Result<Arc<GlobalConfigDbModel>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<GlobalConfigDbModel>>,
    {
        if let Some(value) = self.current() {
            return Ok(value);
        }
        let _fill = self.fill.lock().await;
        // One retry handles a write crossing the first read. Continuous writes fail closed
        // rather than returning an invalidated snapshot or spinning indefinitely.
        for _ in 0..2 {
            if let Some(value) = self.current() {
                return Ok(value);
            }
            let generation = self.state.read().generation;
            let value = Arc::new(load().await?);
            let mut state = self.state.write();
            if state.generation == generation {
                state.value = Some((Instant::now(), value.clone()));
                return Ok(value);
            }
        }
        Err(Error::Other(
            "Global configuration changed during refresh; retry request".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn concurrent_hot_reads_share_one_database_load() {
        let cache = GlobalConfigCache::default();
        let calls = AtomicUsize::new(0);
        let load = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(GlobalConfigDbModel::default())
        };
        let (first, second) = tokio::join!(cache.get_or_load(load), cache.get_or_load(load));
        assert!(Arc::ptr_eq(&first.unwrap(), &second.unwrap()));
        cache.get_or_load(load).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidation_during_load_cannot_publish_or_return_the_old_snapshot() {
        let cache = GlobalConfigCache::default();
        let calls = AtomicUsize::new(0);
        let (release, wait) = tokio::sync::oneshot::channel();
        let wait = Mutex::new(Some(wait));
        let mut read = Box::pin(cache.get_or_load(|| async {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                wait.lock().await.take().unwrap().await.unwrap();
            }
            Ok(GlobalConfigDbModel {
                stream_proxy_allow_private_targets: call == 0,
                ..Default::default()
            })
        }));
        assert!(futures::poll!(read.as_mut()).is_pending());
        cache.invalidate();
        release.send(()).unwrap();
        let value = tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .unwrap()
            .unwrap();
        assert!(!value.stream_proxy_allow_private_targets);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!cache.current().unwrap().stream_proxy_allow_private_targets);
    }

    #[tokio::test]
    async fn expired_or_invalidated_snapshot_is_not_a_fallback_after_load_failure() {
        let cache = GlobalConfigCache::default();
        cache
            .get_or_load(|| async {
                Ok(GlobalConfigDbModel {
                    stream_proxy_allow_private_targets: true,
                    ..Default::default()
                })
            })
            .await
            .unwrap();
        cache.state.write().value.as_mut().unwrap().0 = Instant::now() - GLOBAL_CONFIG_TTL;
        assert!(
            cache
                .get_or_load(|| async { Err(Error::Other("database unavailable".to_string())) })
                .await
                .is_err()
        );
        assert!(cache.current().is_none());
        cache.invalidate();
        assert!(
            cache
                .get_or_load(|| async { Err(Error::Other("database unavailable".to_string())) })
                .await
                .is_err()
        );
        assert!(cache.current().is_none());
    }

    #[tokio::test]
    async fn cancelled_fill_releases_admission_and_repeated_invalidation_is_bounded() {
        let cache = GlobalConfigCache::default();
        let mut fill =
            Box::pin(cache.get_or_load(std::future::pending::<Result<GlobalConfigDbModel>>));
        assert!(futures::poll!(fill.as_mut()).is_pending());
        drop(fill);
        let attempts = AtomicUsize::new(0);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            cache.get_or_load(|| async {
                attempts.fetch_add(1, Ordering::SeqCst);
                cache.invalidate();
                Ok(GlobalConfigDbModel::default())
            }),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(cache.current().is_none());
    }
}
