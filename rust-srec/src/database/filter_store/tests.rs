use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio_util::task::AbortOnDropHandle;

use super::*;
use crate::database::models::FilterDbModel;

struct Gate {
    entered: Notify,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            release: Semaphore::new(0),
        })
    }
}

struct CountingRepository {
    cache: FilterSnapshotCache,
    rows: Mutex<HashMap<String, Vec<FilterDbModel>>>,
    gate: Mutex<Option<Arc<Gate>>>,
    reads: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    fail: AtomicBool,
}

impl CountingRepository {
    fn new(capacity: usize, loads: usize) -> Arc<Self> {
        Arc::new(Self {
            cache: FilterSnapshotCache::with_limits(capacity, loads, SNAPSHOT_TTL),
            rows: Mutex::new(HashMap::new()),
            gate: Mutex::new(None),
            reads: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    }
    fn set(&self, id: &str, value: &str) {
        self.rows.lock().insert(id.into(), vec![keyword(id, value)]);
    }
}

struct ActiveRead<'a>(&'a AtomicUsize);
impl Drop for ActiveRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl FilterRepository for CountingRepository {
    fn filter_snapshot_cache(&self) -> Option<FilterSnapshotCache> {
        Some(self.cache.clone())
    }
    async fn get_filters_for_streamer(&self, id: &str) -> Result<Vec<FilterDbModel>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveRead(&self.active);
        // Capture before the barrier so invalidation can race a genuinely stale
        // database result, not a fake that re-reads new rows when it is released.
        let rows = self.rows.lock().get(id).cloned().unwrap_or_default();
        let fail = self.fail.load(Ordering::SeqCst);
        let gate = self.gate.lock().clone();
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.acquire().await.unwrap().forget();
        }
        if fail {
            Err(Error::DatabaseSqlx(sqlx::Error::RowNotFound))
        } else {
            Ok(rows)
        }
    }
    async fn get_filter(&self, _id: &str) -> Result<FilterDbModel> {
        Err(Error::Other("unused read".into()))
    }
    async fn create_filter(&self, _filter: &FilterDbModel) -> Result<()> {
        Err(Error::Other("read-only fixture".into()))
    }
    async fn update_filter(&self, _filter: &FilterDbModel) -> Result<()> {
        Err(Error::Other("read-only fixture".into()))
    }
    async fn delete_filter(&self, _id: &str) -> Result<()> {
        Err(Error::Other("read-only fixture".into()))
    }
    async fn delete_filters_for_streamer(&self, _id: &str) -> Result<()> {
        Err(Error::Other("read-only fixture".into()))
    }
}

fn keyword(id: &str, value: &str) -> FilterDbModel {
    FilterDbModel {
        id: format!("filter-{id}"),
        streamer_id: id.into(),
        filter_type: "KEYWORD".into(),
        config: serde_json::json!({"include": [value], "exclude": []}).to_string(),
    }
}

fn value(snapshot: &Snapshot) -> &str {
    let Filter::Keyword(filter) = &snapshot[0] else {
        panic!("expected keyword snapshot")
    };
    &filter.include[0]
}

async fn poll_pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn repeated_concurrent_and_empty_snapshots_share_one_repository_read() {
    tokio::time::timeout(Duration::from_secs(3), async {
        for empty in [false, true] {
            let repository = CountingRepository::new(8, 2);
            if !empty {
                repository.set("one", "original");
            }
            let gate = Gate::new();
            *repository.gate.lock() = Some(gate.clone());
            let store = Arc::new(FilterStore::new(repository.clone()));
            let task_store = store.clone();
            let leader =
                AbortOnDropHandle::new(tokio::spawn(async move { task_store.get("one").await }));
            gate.entered.notified().await;
            let mut followers: Vec<_> = (0..24).map(|_| Box::pin(store.get("one"))).collect();
            for follower in &mut followers {
                poll_pending(follower.as_mut()).await;
            }
            assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
            gate.release.add_permits(1);
            let snapshot = leader.await.unwrap().unwrap();
            assert_eq!(snapshot.is_empty(), empty);
            for follower in followers {
                assert!(Arc::ptr_eq(&snapshot, &follower.await.unwrap()));
            }
            assert!(Arc::ptr_eq(&snapshot, &store.get("one").await.unwrap()));
            assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
        }
    })
    .await
    .expect("coalesced snapshots must settle");
}

#[tokio::test]
async fn cancelled_and_failed_leaders_release_waiters_and_allow_retry() {
    tokio::time::timeout(Duration::from_secs(3), async {
        for cancelled in [false, true] {
            let repository = CountingRepository::new(8, 1);
            let gate = Gate::new();
            *repository.gate.lock() = Some(gate.clone());
            repository.fail.store(!cancelled, Ordering::SeqCst);
            let store = Arc::new(FilterStore::new(repository.clone()));
            let task_store = store.clone();
            let leader = tokio::spawn(async move { task_store.get("one").await });
            gate.entered.notified().await;
            let follower = store.get("one");
            tokio::pin!(follower);
            poll_pending(follower.as_mut()).await;
            let leader_error = if cancelled {
                leader.abort();
                assert!(leader.await.unwrap_err().is_cancelled());
                None
            } else {
                gate.release.add_permits(1);
                Some(leader.await.unwrap().unwrap_err())
            };
            let follower_error = follower.await.unwrap_err();
            if let Some(leader_error) = leader_error {
                let (
                    Error::FilterSnapshot(FilterSnapshotError::Repository(leader_source)),
                    Error::FilterSnapshot(FilterSnapshotError::Repository(follower_source)),
                ) = (&leader_error, &follower_error)
                else {
                    panic!("both readers must retain the repository failure");
                };
                assert!(Arc::ptr_eq(leader_source, follower_source));
                assert!(matches!(
                    follower_source.as_ref(),
                    Error::DatabaseSqlx(sqlx::Error::RowNotFound)
                ));
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&follower_error);
                let mut found_sqlx = false;
                while let Some(error) = source {
                    found_sqlx |= matches!(
                        error.downcast_ref::<sqlx::Error>(),
                        Some(sqlx::Error::RowNotFound)
                    );
                    source = error.source();
                }
                assert!(
                    found_sqlx,
                    "diagnostics must retain the original source chain"
                );
            } else {
                assert!(matches!(
                    &follower_error,
                    Error::FilterSnapshot(FilterSnapshotError::Cancelled)
                ));
            }
            assert!(
                crate::scheduler::actor::CheckError::from(follower_error).transient,
                "the monitor adapter keeps repository/cancellation failures transient"
            );
            assert_eq!(repository.active.load(Ordering::SeqCst), 0);
            assert!(repository.cache.inner.state.lock().loads.is_empty());
            *repository.gate.lock() = None;
            repository.fail.store(false, Ordering::SeqCst);
            repository.set("one", "retried");
            assert_eq!(value(&store.get("one").await.unwrap()), "retried");
            assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
        }
    })
    .await
    .expect("failed ownership cannot strand filter waiters");
}

#[tokio::test(start_paused = true)]
async fn hung_repository_is_bounded_and_cached_out_of_band_staleness_expires_at_ttl() {
    let repository = CountingRepository::new(8, 1);
    let gate = Gate::new();
    *repository.gate.lock() = Some(gate);
    let store = FilterStore::new(repository.clone());
    let load = store.get("one");
    tokio::pin!(load);
    poll_pending(load.as_mut()).await;
    tokio::time::advance(LOAD_TIMEOUT).await;
    let error = load.await.unwrap_err();
    assert!(matches!(
        &error,
        Error::FilterSnapshot(FilterSnapshotError::TimedOut)
    ));
    assert!(crate::scheduler::actor::CheckError::from(error).transient);
    *repository.gate.lock() = None;
    repository.set("one", "before");
    let original = store.get("one").await.unwrap();
    repository.set("one", "out-of-band");
    tokio::time::advance(SNAPSHOT_TTL - Duration::from_millis(1)).await;
    assert!(Arc::ptr_eq(&original, &store.get("one").await.unwrap()));
    tokio::time::advance(Duration::from_millis(1)).await;
    let refreshed = store.get("one").await.unwrap();
    assert_eq!(value(&refreshed), "out-of-band");
    assert_eq!(
        value(&original),
        "before",
        "in-progress checks retain their immutable snapshot"
    );
    assert_eq!(repository.reads.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn invalidated_delayed_load_cannot_repopulate_or_override_its_successor() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let repository = CountingRepository::new(8, 2);
        repository.set("one", "old");
        let gate = Gate::new();
        *repository.gate.lock() = Some(gate.clone());
        let store = Arc::new(FilterStore::new(repository.clone()));
        let task_store = store.clone();
        let leader =
            AbortOnDropHandle::new(tokio::spawn(async move { task_store.get("one").await }));
        gate.entered.notified().await;
        let old_load = repository.cache.inner.state.lock().loads["one"].clone();
        let follower = store.get("one");
        tokio::pin!(follower);
        poll_pending(follower.as_mut()).await;
        repository.set("one", "new");
        *repository.gate.lock() = None;
        store.invalidate("one");
        let fresh = follower.await.unwrap();
        assert_eq!(value(&fresh), "new");
        assert!(Arc::ptr_eq(&fresh, &leader.await.unwrap().unwrap()));
        // Even a completion that was already resolved when invalidation won
        // cannot publish through its retired identity.
        let old: Snapshot = vec![Filter::try_from(&keyword("one", "old")).unwrap()].into();
        assert!(matches!(
            repository
                .cache
                .publish("one", &old_load, LoadResult::Ready(old)),
            LoadResult::Invalidated
        ));
        assert!(Arc::ptr_eq(&fresh, &store.get("one").await.unwrap()));
        assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
    })
    .await
    .expect("invalidated loads must converge on current rows");
}

#[tokio::test]
async fn cache_entries_and_inflight_repository_work_remain_bounded() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let repository = CountingRepository::new(2, 2);
        let gate = Gate::new();
        *repository.gate.lock() = Some(gate.clone());
        let store = Arc::new(FilterStore::new(repository.clone()));
        let tasks: Vec<_> = (0..12)
            .map(|id| {
                let store = store.clone();
                AbortOnDropHandle::new(tokio::spawn(
                    async move { store.get(&id.to_string()).await },
                ))
            })
            .collect();
        while repository.reads.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
        assert_eq!(repository.cache.inner.state.lock().loads.len(), 2);
        gate.release.add_permits(12);
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(repository.reads.load(Ordering::SeqCst), 12);
        assert_eq!(repository.peak.load(Ordering::SeqCst), 2);
        assert_eq!(repository.cache.inner.state.lock().entries.len(), 2);
        *repository.gate.lock() = None;
        store.invalidate_all();
        store.get("a").await.unwrap();
        store.get("b").await.unwrap();
        store.get("a").await.unwrap();
        store.get("c").await.unwrap();
        let before = repository.reads.load(Ordering::SeqCst);
        store.get("a").await.unwrap();
        assert_eq!(repository.reads.load(Ordering::SeqCst), before);
        store.get("b").await.unwrap();
        assert_eq!(repository.reads.load(Ordering::SeqCst), before + 1);
    })
    .await
    .expect("filter cache capacity cannot grow with requester count");
}

#[tokio::test]
async fn parsed_snapshots_preserve_order_and_skip_invalid_rows() {
    let repository = CountingRepository::new(8, 2);
    let mut invalid = keyword("one", "invalid");
    invalid.config = "malformed JSON".into();
    repository.rows.lock().insert(
        "one".into(),
        vec![keyword("one", "first"), invalid, keyword("one", "last")],
    );
    let store = FilterStore::new(repository.clone());
    let filters = store.get("one").await.unwrap();
    assert_eq!(filters.len(), 2);
    assert_eq!(value(&filters), "first");
    let Filter::Keyword(last) = &filters[1] else {
        panic!("expected last keyword")
    };
    assert_eq!(last.include, vec!["last"]);
    assert!(Arc::ptr_eq(&filters, &store.get("one").await.unwrap()));
}
