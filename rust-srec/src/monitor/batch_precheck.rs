//! Answers routine offline polls from a platform's batch status call.
//!
//! Streamers whose platform has a
//! [`BatchStatusProbe`](platforms_parser::extractor::batch_status::BatchStatusProbe) are grouped by
//! platform and network route, and each group is split into chunks of at most the probe's
//! `max_keys` members. A chunk keeps the result of its latest batch call. A poll that finds its
//! streamer offline in a recent enough result is answered without the platform's extractor; a
//! streamer reported live, one the result does not mention, and every poll while the chunk has no
//! usable result run the individual check instead.
//!
//! A result is reused for a quarter of the poll's interval. A poll that waited for its chunk's
//! in-flight refresh reads that refresh's result whatever its age, so an answer is at most a quarter
//! of the interval or the duration of that call (bounded by the request timeout) old, whichever is
//! longer.
//!
//! Polling is the registration. A streamer joins the first chunk with a free slot the first time it
//! polls and keeps that slot while it keeps polling. One that stops (deleted, disabled, recording,
//! backing off, moved to another route) is dropped once it has gone [`EXPIRY_INTERVALS`] check
//! intervals (at least [`MIN_EXPIRY`]) without a poll: its chunk's next batch call leaves it out, and
//! the next streamer to join the group can take its slot.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use platforms_parser::extractor::batch_status::BatchPresence;
use platforms_parser::extractor::error::ExtractorError;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::proxies::RouteKey;

/// The first wait after a failed batch call. Each further consecutive failure doubles it, up to
/// [`MAX_BACKOFF`]. A throttle that states how long to wait extends the wait to that, also up to
/// [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);

/// A member that has not polled for this many check intervals gives up its slot.
const EXPIRY_INTERVALS: u32 = 4;

/// The shortest time a silent member keeps its slot, so a poll delayed by a busy scheduler or a
/// slow check does not cost the streamer its place under a short check interval.
const MIN_EXPIRY: Duration = Duration::from_secs(60);

/// Presence by batch key, as a batch call reports it.
pub(crate) type BatchResult = HashMap<String, BatchPresence>;

/// Streamers that share one batch call: the same platform probe, the same admission limiter and
/// the same network route.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GroupKey {
    /// The platform name the URL registry reports; each platform has at most one probe.
    pub platform: &'static str,
    /// The platform configuration whose admission limiter the batch call passes.
    pub platform_id: String,
    pub route: RouteKey,
}

/// One streamer's poll.
pub(crate) struct Poll<'a> {
    pub group: GroupKey,
    pub streamer_id: &'a str,
    /// The streamer's key in batch calls.
    pub key: String,
    /// The most keys one batch call of the platform accepts.
    pub max_keys: usize,
    /// How often the streamer is polled. A cached result answers it for a quarter of this.
    pub interval: Duration,
}

/// The cached batch results, shared by every poll of the monitor.
#[derive(Default)]
pub(crate) struct BatchPrecheck {
    groups: Mutex<HashMap<GroupKey, Vec<Arc<Chunk>>>>,
}

#[derive(Default)]
struct Chunk {
    state: Mutex<ChunkState>,
    /// Held while the chunk's batch call runs, so the chunk makes one call at a time and polls
    /// arriving meanwhile wait for its result.
    refreshing: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct ChunkState {
    members: HashMap<String, Member>,
    result: Option<Cached>,
    /// Finished batch calls, successful or not. A poll that waited for a refresh reads its
    /// outcome instead of starting another.
    refreshes: u64,
    /// Batch calls that failed in a row.
    failures: u32,
    backoff_until: Option<Instant>,
}

struct Member {
    key: String,
    last_poll: Instant,
}

struct Cached {
    /// When the call that produced the result started; the platform answered at some point after.
    at: Instant,
    presence: BatchResult,
}

enum Read {
    Answer(bool),
    /// The chunk needs a new batch call; the value is its refresh count at the time.
    Stale(u64),
}

/// How long a member keeps its slot without polling, for polls every `interval`.
fn expiry(interval: Duration) -> Duration {
    interval.saturating_mul(EXPIRY_INTERVALS).max(MIN_EXPIRY)
}

impl ChunkState {
    fn read(&self, key: &str, interval: Duration, now: Instant) -> Read {
        if self.backoff_until.is_some_and(|until| now < until) {
            return Read::Answer(false);
        }
        match &self.result {
            Some(cached) if now.saturating_duration_since(cached.at) <= interval / 4 => {
                Read::Answer(self.offline(key))
            }
            _ => Read::Stale(self.refreshes),
        }
    }

    /// Whether the cached result reports `key` offline. A key the result does not mention, one it
    /// reports live, and a chunk without a result all mean the individual check must decide.
    fn offline(&self, key: &str) -> bool {
        self.result
            .as_ref()
            .is_some_and(|cached| cached.presence.get(key) == Some(&BatchPresence::Offline))
    }

    fn expire_members(&mut self, now: Instant, interval: Duration) {
        let expiry = expiry(interval);
        self.members
            .retain(|_, member| now.saturating_duration_since(member.last_poll) <= expiry);
    }
}

fn backoff_after(failures: u32, error: &crate::Error) -> Duration {
    let doublings = failures.saturating_sub(1).min(5);
    let exponential = FIRST_BACKOFF.saturating_mul(1 << doublings);
    let requested = match error {
        crate::Error::Extractor(ExtractorError::RateLimited {
            retry_after: Some(retry_after),
            ..
        }) => *retry_after,
        _ => Duration::ZERO,
    };
    exponential.max(requested).min(MAX_BACKOFF)
}

impl BatchPrecheck {
    /// Whether the batch result of the poll's chunk reports the streamer offline.
    ///
    /// A result no older than a quarter of the poll's interval is reused. Otherwise this poll
    /// refreshes the chunk with `call`, which receives the chunk's distinct keys (at most
    /// `max_keys`), and reads the new result. Polls of a chunk that is already refreshing wait for
    /// that refresh instead. A failed call backs the chunk off, and its polls answer `false` until
    /// the backoff ends.
    pub(crate) async fn offline<F, Fut>(&self, poll: Poll<'_>, call: F) -> bool
    where
        F: FnOnce(Vec<String>) -> Fut,
        Fut: Future<Output = crate::Result<BatchResult>>,
    {
        let chunk = self.join(&poll);
        let seen = match chunk
            .state
            .lock()
            .read(&poll.key, poll.interval, Instant::now())
        {
            Read::Answer(offline) => return offline,
            Read::Stale(seen) => seen,
        };

        // A cancelled refresh releases this lock without counting as finished, so the next
        // waiter makes the call itself.
        let _refreshing = chunk.refreshing.lock().await;
        let keys = {
            let mut state = chunk.state.lock();
            if state.refreshes != seen {
                return state.offline(&poll.key);
            }
            state.expire_members(Instant::now(), poll.interval);
            let mut distinct = HashSet::new();
            state
                .members
                .values()
                .filter(|member| distinct.insert(member.key.as_str()))
                .map(|member| member.key.clone())
                .collect::<Vec<_>>()
        };

        let chunk_size = keys.len();
        let started = Instant::now();
        let outcome = call(keys).await;
        let mut state = chunk.state.lock();
        state.refreshes += 1;
        match outcome {
            Ok(presence) => {
                debug!(
                    platform = poll.group.platform,
                    route = ?poll.group.route,
                    chunk_size,
                    live = presence
                        .values()
                        .filter(|presence| **presence == BatchPresence::Live)
                        .count(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "Batch status refreshed"
                );
                state.failures = 0;
                state.backoff_until = None;
                state.result = Some(Cached {
                    at: started,
                    presence,
                });
                state.offline(&poll.key)
            }
            Err(error) => {
                state.failures = state.failures.saturating_add(1);
                let backoff = backoff_after(state.failures, &error);
                state.backoff_until = Some(Instant::now() + backoff);
                state.result = None;
                if state.failures == 1 {
                    warn!(
                        platform = poll.group.platform,
                        route = ?poll.group.route,
                        chunk_size,
                        backoff_secs = backoff.as_secs(),
                        %error,
                        "Batch status call failed; checking these streamers individually meanwhile"
                    );
                } else {
                    debug!(
                        platform = poll.group.platform,
                        route = ?poll.group.route,
                        failures = state.failures,
                        backoff_secs = backoff.as_secs(),
                        %error,
                        "Batch status call failed again"
                    );
                }
                false
            }
        }
    }

    /// The chunk the poll's streamer belongs to, joining one if it has none in the poll's group.
    fn join(&self, poll: &Poll<'_>) -> Arc<Chunk> {
        let now = Instant::now();
        let mut groups = self.groups.lock();
        let chunks = groups.entry(poll.group.clone()).or_default();
        for chunk in chunks.iter() {
            if let Some(member) = chunk.state.lock().members.get_mut(poll.streamer_id) {
                member.key.clone_from(&poll.key);
                member.last_poll = now;
                return chunk.clone();
            }
        }

        for chunk in chunks.iter() {
            chunk.state.lock().expire_members(now, poll.interval);
        }
        chunks.retain(|chunk| !chunk.state.lock().members.is_empty());
        let capacity = poll.max_keys.max(1);
        let chunk = match chunks
            .iter()
            .find(|chunk| chunk.state.lock().members.len() < capacity)
        {
            Some(chunk) => chunk.clone(),
            None => {
                let chunk = Arc::new(Chunk::default());
                chunks.push(chunk.clone());
                chunk
            }
        };
        chunk.state.lock().members.insert(
            poll.streamer_id.to_owned(),
            Member {
                key: poll.key.clone(),
                last_poll: now,
            },
        );
        chunk
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const INTERVAL: Duration = Duration::from_secs(60);

    fn poll(streamer: &str, max_keys: usize) -> Poll<'_> {
        Poll {
            group: GroupKey {
                platform: "Fake",
                platform_id: "platform-fake".to_owned(),
                route: RouteKey::Direct,
            },
            streamer_id: streamer,
            key: format!("key-{streamer}"),
            max_keys,
            interval: INTERVAL,
        }
    }

    /// Records the keys of every batch call and answers each key offline, or fails while
    /// `failing` is set.
    #[derive(Default)]
    struct FakeCalls {
        calls: Mutex<Vec<Vec<String>>>,
        failing: std::sync::atomic::AtomicBool,
    }

    impl FakeCalls {
        async fn call(&self, keys: Vec<String>) -> crate::Result<BatchResult> {
            self.calls.lock().push(keys.clone());
            if self.failing.load(Ordering::SeqCst) {
                return Err(crate::Error::Monitor("batch call failed".into()));
            }
            Ok(keys
                .into_iter()
                .map(|key| (key, BatchPresence::Offline))
                .collect())
        }

        fn count(&self) -> usize {
            self.calls.lock().len()
        }

        fn last(&self) -> Vec<String> {
            let mut keys = self.calls.lock().last().cloned().unwrap_or_default();
            keys.sort();
            keys
        }

        fn fail(&self, failing: bool) {
            self.failing.store(failing, Ordering::SeqCst);
        }
    }

    async fn offline(precheck: &BatchPrecheck, calls: &FakeCalls, poll: Poll<'_>) -> bool {
        precheck.offline(poll, |keys| calls.call(keys)).await
    }

    #[tokio::test(start_paused = true)]
    async fn members_beyond_the_limit_form_another_chunk() {
        let precheck = BatchPrecheck::default();
        let calls = FakeCalls::default();
        let streamers: Vec<String> = (0..101).map(|index| format!("s{index:03}")).collect();
        for streamer in &streamers {
            offline(&precheck, &calls, poll(streamer, 100)).await;
        }
        tokio::time::advance(INTERVAL).await;
        let before = calls.count();
        for streamer in &streamers {
            assert!(offline(&precheck, &calls, poll(streamer, 100)).await);
        }

        let round: Vec<Vec<String>> = calls.calls.lock()[before..].to_vec();
        let mut sizes: Vec<usize> = round.iter().map(Vec::len).collect();
        sizes.sort();
        assert_eq!(sizes, [1, 100]);
        let covered: HashSet<&String> = round.iter().flatten().collect();
        assert_eq!(covered.len(), 101);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_polls_of_a_stale_chunk_share_one_call() {
        let precheck = BatchPrecheck::default();
        let calls = AtomicUsize::new(0);
        let slow_call = |keys: Vec<String>| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok::<_, crate::Error>(
                    keys.into_iter()
                        .map(|key| (key, BatchPresence::Offline))
                        .collect(),
                )
            }
        };
        for streamer in ["a", "b", "c"] {
            precheck.offline(poll(streamer, 100), slow_call).await;
        }
        tokio::time::advance(INTERVAL).await;
        calls.store(0, Ordering::SeqCst);

        let answers = futures::future::join_all(
            ["a", "b", "c"].map(|streamer| precheck.offline(poll(streamer, 100), slow_call)),
        )
        .await;
        assert_eq!(answers, [true, true, true]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_result_serves_polls_for_a_quarter_of_their_interval() {
        let precheck = BatchPrecheck::default();
        let calls = FakeCalls::default();
        assert!(offline(&precheck, &calls, poll("a", 100)).await);
        assert_eq!(calls.count(), 1);

        tokio::time::advance(INTERVAL / 4).await;
        assert!(offline(&precheck, &calls, poll("a", 100)).await);
        assert_eq!(
            calls.count(),
            1,
            "a result a quarter-interval old is reused"
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(offline(&precheck, &calls, poll("a", 100)).await);
        assert_eq!(calls.count(), 2, "an older result is refreshed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_call_backs_off_its_own_chunk_with_a_growing_delay() {
        let precheck = BatchPrecheck::default();
        let calls = FakeCalls::default();
        // One key per call puts each streamer in its own chunk.
        assert!(offline(&precheck, &calls, poll("ok", 1)).await);
        tokio::time::advance(INTERVAL).await;

        calls.fail(true);
        assert!(!offline(&precheck, &calls, poll("failing", 1)).await);
        assert!(!offline(&precheck, &calls, poll("failing", 1)).await);
        assert_eq!(calls.count(), 2, "no call during the backoff");
        calls.fail(false);
        assert!(
            offline(&precheck, &calls, poll("ok", 1)).await,
            "other chunks keep their batch calls"
        );
        assert_eq!(calls.count(), 3);

        // The second failure in a row doubles the wait.
        calls.fail(true);
        tokio::time::advance(FIRST_BACKOFF).await;
        assert!(!offline(&precheck, &calls, poll("failing", 1)).await);
        assert_eq!(calls.count(), 4);
        tokio::time::advance(FIRST_BACKOFF * 2 - Duration::from_millis(1)).await;
        assert!(!offline(&precheck, &calls, poll("failing", 1)).await);
        assert_eq!(calls.count(), 4);

        // A success ends the run, so the next failure waits the first delay again.
        calls.fail(false);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(offline(&precheck, &calls, poll("failing", 1)).await);
        assert_eq!(calls.count(), 5);
        calls.fail(true);
        tokio::time::advance(INTERVAL).await;
        assert!(!offline(&precheck, &calls, poll("failing", 1)).await);
        tokio::time::advance(FIRST_BACKOFF).await;
        calls.fail(false);
        assert!(offline(&precheck, &calls, poll("failing", 1)).await);
        assert_eq!(calls.count(), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn a_member_that_stops_polling_gives_up_its_slot() {
        let precheck = BatchPrecheck::default();
        let calls = FakeCalls::default();
        for streamer in ["a", "silent"] {
            offline(&precheck, &calls, poll(streamer, 2)).await;
        }
        // "a" keeps polling while "silent" stops for longer than its expiry.
        for _ in 0..EXPIRY_INTERVALS {
            tokio::time::advance(INTERVAL).await;
            offline(&precheck, &calls, poll("a", 2)).await;
        }
        tokio::time::advance(Duration::from_millis(1)).await;

        // "d" takes the freed slot. The result "a" just fetched does not cover it yet, so its
        // first poll is checked individually; the chunk's next call includes it.
        assert!(!offline(&precheck, &calls, poll("d", 2)).await);
        tokio::time::advance(INTERVAL).await;
        assert!(offline(&precheck, &calls, poll("d", 2)).await);
        assert_eq!(calls.last(), ["key-a", "key-d"]);
    }
}
