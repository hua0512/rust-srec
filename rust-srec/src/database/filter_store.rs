//! Bounded immutable filter snapshots shared by readers and committed mutations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::domain::filter::Filter;
use crate::{Error, Result};

use super::repositories::FilterRepository;

const MAX_SNAPSHOTS: usize = 1024;
const MAX_LOADS: usize = 16;
const SNAPSHOT_TTL: Duration = Duration::from_secs(30);
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

type Snapshot = Arc<[Filter]>;

/// Cloneable load failures preserve the repository error and its source chain
/// for every coalesced reader. Invalidation remains a separate retry outcome.
#[derive(Clone, Debug, thiserror::Error)]
pub enum FilterSnapshotError {
    #[error("{0}")]
    Repository(#[source] Arc<Error>),
    #[error("filter snapshot load timed out")]
    TimedOut,
    #[error("filter snapshot load cancelled")]
    Cancelled,
    #[error("filter snapshot owner stopped")]
    OwnerStopped,
    #[error("filter snapshot admission closed")]
    AdmissionClosed,
}

#[derive(Clone)]
enum LoadResult {
    Ready(Snapshot),
    Failed(FilterSnapshotError),
    Invalidated,
}

struct Load {
    result: watch::Sender<Option<LoadResult>>,
    invalidated: CancellationToken,
    started_at: Instant,
}

impl Load {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: watch::channel(None).0,
            invalidated: CancellationToken::new(),
            started_at: Instant::now(),
        })
    }

    async fn wait(&self) -> LoadResult {
        let mut result = self.result.subscribe();
        loop {
            let current = result.borrow().clone();
            if let Some(current) = current {
                return current;
            }
            if result.changed().await.is_err() {
                return LoadResult::Failed(FilterSnapshotError::OwnerStopped);
            }
        }
    }
}

struct Entry {
    filters: Snapshot,
    expires_at: Instant,
    last_used: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    loads: HashMap<String, Arc<Load>>,
    clock: u64,
}

struct CacheInner {
    state: Mutex<State>,
    permits: Arc<Semaphore>,
    capacity: usize,
    ttl: Duration,
}

/// Shared invalidation boundary for a repository and all stores reading it.
///
/// Invalidation does not mutate snapshots already held by checks. It prevents
/// any older in-flight load from publishing and forces subsequent checks to load
/// the new rows. Empty snapshots consume one bounded cache entry too.
#[derive(Clone)]
pub struct FilterSnapshotCache {
    inner: Arc<CacheInner>,
}

impl Default for FilterSnapshotCache {
    fn default() -> Self {
        Self::with_limits(MAX_SNAPSHOTS, MAX_LOADS, SNAPSHOT_TTL)
    }
}

impl FilterSnapshotCache {
    fn with_limits(capacity: usize, loads: usize, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(CacheInner {
                state: Mutex::new(State::default()),
                permits: Arc::new(Semaphore::new(loads)),
                capacity,
                ttl,
            }),
        }
    }

    pub fn invalidate(&self, streamer_id: &str) {
        let mut state = self.inner.state.lock();
        state.entries.remove(streamer_id);
        if let Some(load) = state.loads.remove(streamer_id) {
            load.result.send_replace(Some(LoadResult::Invalidated));
            load.invalidated.cancel();
        }
    }

    pub fn invalidate_all(&self) {
        let mut state = self.inner.state.lock();
        state.entries.clear();
        for (_, load) in state.loads.drain() {
            load.result.send_replace(Some(LoadResult::Invalidated));
            load.invalidated.cancel();
        }
    }

    fn lookup_locked(state: &mut State, id: &str) -> Lookup {
        state.clock = state.clock.wrapping_add(1);
        if let Some(entry) = state.entries.get_mut(id) {
            if entry.expires_at > Instant::now() {
                entry.last_used = state.clock;
                return Lookup::Ready(entry.filters.clone());
            }
            state.entries.remove(id);
        }
        match state.loads.get(id) {
            Some(load) => Lookup::Waiting(load.clone()),
            None => Lookup::Missing,
        }
    }

    fn lookup(&self, id: &str) -> Lookup {
        Self::lookup_locked(&mut self.inner.state.lock(), id)
    }

    fn publish(&self, id: &str, load: &Arc<Load>, result: LoadResult) -> LoadResult {
        let mut state = self.inner.state.lock();
        // Identity comparison and insertion are one critical section. A mutation
        // cannot slip between a successful generation check and stale insertion.
        if !state
            .loads
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, load))
        {
            return LoadResult::Invalidated;
        }
        state.loads.remove(id);
        if let LoadResult::Ready(filters) = &result {
            state
                .entries
                .retain(|_, entry| entry.expires_at > Instant::now());
            if state.entries.len() >= self.inner.capacity
                && let Some(oldest) = state
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(id, _)| id.clone())
            {
                state.entries.remove(&oldest);
            }
            let last_used = state.clock;
            // Count TTL from load admission, not completion: a slow query must
            // not extend out-of-band staleness by another query duration.
            state.entries.insert(
                id.to_owned(),
                Entry {
                    filters: filters.clone(),
                    expires_at: load.started_at + self.inner.ttl,
                    last_used,
                },
            );
        }
        load.result.send_replace(Some(result.clone()));
        result
    }
}

enum Lookup {
    Ready(Snapshot),
    Waiting(Arc<Load>),
    Missing,
}

struct LoadGuard {
    cache: FilterSnapshotCache,
    id: String,
    load: Arc<Load>,
    _permit: OwnedSemaphorePermit,
    completed: bool,
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cache.publish(
                &self.id,
                &self.load,
                LoadResult::Failed(FilterSnapshotError::Cancelled),
            );
        }
    }
}

/// Coalesces repository reads without adding a filter-repository generic to
/// configuration consumers. Capacity waiters do not allocate cache/load entries.
pub struct FilterStore {
    repository: Arc<dyn FilterRepository>,
    cache: FilterSnapshotCache,
}

impl FilterStore {
    pub fn new(repository: Arc<dyn FilterRepository>) -> Self {
        let cache = repository.filter_snapshot_cache().unwrap_or_default();
        Self { repository, cache }
    }

    pub fn invalidate(&self, streamer_id: &str) {
        self.cache.invalidate(streamer_id);
    }
    pub fn invalidate_all(&self) {
        self.cache.invalidate_all();
    }

    pub async fn get(&self, streamer_id: &str) -> Result<Snapshot> {
        loop {
            let result = match self.cache.lookup(streamer_id) {
                Lookup::Ready(filters) => return Ok(filters),
                Lookup::Waiting(load) => load.wait().await,
                Lookup::Missing => {
                    let permit = self
                        .cache
                        .inner
                        .permits
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| FilterSnapshotError::AdmissionClosed)?;
                    // A different caller may have loaded this id while admission
                    // waited. Recheck under the same lock that installs the leader.
                    let guard = {
                        let mut state = self.cache.inner.state.lock();
                        match FilterSnapshotCache::lookup_locked(&mut state, streamer_id) {
                            Lookup::Missing => {
                                let load = Load::new();
                                state.loads.insert(streamer_id.to_owned(), load.clone());
                                Some(LoadGuard {
                                    cache: self.cache.clone(),
                                    id: streamer_id.to_owned(),
                                    load,
                                    _permit: permit,
                                    completed: false,
                                })
                            }
                            _ => None,
                        }
                    };
                    let Some(mut guard) = guard else {
                        continue;
                    };
                    let rows = tokio::select! {
                        biased;
                        _ = guard.load.invalidated.cancelled() => { continue; }
                        rows = tokio::time::timeout(LOAD_TIMEOUT, self.repository.get_by_streamer(streamer_id)) => rows,
                    };
                    let result = match rows {
                        Ok(Ok(rows)) => {
                            let filters: Vec<_> = rows.iter().filter_map(|model| match Filter::try_from(model) {
                                Ok(filter) => Some(filter),
                                Err(error) => {
                                    warn!(filter_id = %model.id, filter_type = %model.filter_type, %error, "Skipping invalid streamer filter");
                                    None
                                }
                            }).collect();
                            LoadResult::Ready(filters.into())
                        }
                        Ok(Err(error)) => {
                            LoadResult::Failed(FilterSnapshotError::Repository(Arc::new(error)))
                        }
                        Err(_) => LoadResult::Failed(FilterSnapshotError::TimedOut),
                    };
                    let result = self.cache.publish(streamer_id, &guard.load, result);
                    guard.completed = true;
                    result
                }
            };
            match result {
                LoadResult::Ready(filters) => return Ok(filters),
                LoadResult::Failed(error) => return Err(error.into()),
                LoadResult::Invalidated => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation_tests;
