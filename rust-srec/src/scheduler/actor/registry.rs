//! Actor registry for tracking and managing actors.
//!
//! The `ActorRegistry` provides centralized management of actors:
//! - Tracks streamer actors by ID
//! - Integrates with `JoinSet` for task management
//! - Supports actor spawning and removal
//! - Provides actor lookup and enumeration

use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::handle::{ActorHandle, ActorStopSignal};
use super::messages::StreamerMessage;
use super::streamer_actor::{ActorError, ActorOutcome, ActorResult, StreamerActor};

/// Result of an actor task completion.
#[derive(Debug)]
pub struct ActorTaskResult {
    /// Actor ID.
    pub actor_id: String,
    /// Registry generation the finished task was spawned under.
    pub generation: u64,
    /// The outcome of the actor's run.
    pub outcome: ActorResult,
}

impl ActorTaskResult {
    /// Create a result for a streamer actor.
    pub fn streamer(id: impl Into<String>, generation: u64, outcome: ActorResult) -> Self {
        Self {
            actor_id: id.into(),
            generation,
            outcome,
        }
    }

    /// Check if the actor crashed with a recoverable failure rather than a terminal stop.
    pub fn is_crash(&self) -> bool {
        match &self.outcome {
            Ok(ActorOutcome::Stopped) | Ok(ActorOutcome::Cancelled) => false,
            Ok(ActorOutcome::Completed) => false,
            Err(error) => error.recoverable,
        }
    }

    /// Get the error message if this was a crash.
    pub fn error_message(&self) -> Option<&str> {
        match &self.outcome {
            Err(ActorError { message, .. }) => Some(message.as_str()),
            _ => None,
        }
    }
}

/// Registry for tracking and managing actors.
///
/// The registry maintains handles to all active actors and integrates
/// with a `JoinSet` for task lifecycle management.
pub struct ActorRegistry {
    /// Streamer actors by ID.
    streamers: HashMap<String, ActorHandle<StreamerMessage>>,
    /// Advances by exactly one per streamer membership change (insert, remove,
    /// or clear) and never otherwise. Scheduler feedback relies on the
    /// single step to apply one named change without rescanning the map.
    streamer_revision: u64,
    /// JoinSet for actor tasks.
    task_set: JoinSet<ActorTaskResult>,
    /// Parent cancellation token.
    cancellation_token: CancellationToken,
    /// Source of the generation stamped onto every spawned actor. Starts at 0
    /// so `next_generation` never hands out the "unregistered" value 0.
    generation_counter: u64,
    /// Stop signals of streamer tasks that are no longer in `streamers` but may
    /// still be running, keyed by actor ID.
    ///
    /// `remove_streamer` drops the handle while the task keeps going until its
    /// in-flight `check_status` returns, which would otherwise put the only copy
    /// of the signal out of reach and make a later `remove_streamer_awaitable`
    /// for the same ID report `ActorRemovalOutcome::NotRegistered` for an actor
    /// that is still writing streamer state. Entries are added by
    /// `remove_streamer`, dropped by `handle_task_completion` when it reconciles
    /// that generation, pruned whenever their signal has fired, and cleared by
    /// `clear`, so the map holds at most one entry per removed-but-running task.
    unreaped_streamers: HashMap<String, Vec<UnreapedActor>>,
}

/// A streamer task the registry has stopped tracking but not yet reconciled.
#[derive(Debug, Clone)]
struct UnreapedActor {
    /// Generation the task was spawned under, matched against
    /// `ActorTaskResult::generation` in `handle_task_completion`.
    generation: u64,
    stop_signal: ActorStopSignal,
}

impl ActorRegistry {
    /// Create a new empty registry.
    pub fn new(cancellation_token: CancellationToken) -> Self {
        Self {
            streamers: HashMap::new(),
            streamer_revision: 0,
            task_set: JoinSet::new(),
            cancellation_token,
            generation_counter: 0,
            unreaped_streamers: HashMap::new(),
        }
    }

    /// Allocate the next generation for a spawn.
    fn next_generation(&mut self) -> u64 {
        self.generation_counter += 1;
        self.generation_counter
    }

    /// Get the number of streamer actors.
    pub fn streamer_count(&self) -> usize {
        self.streamers.len()
    }

    /// Check if a streamer actor exists.
    pub fn has_streamer(&self, id: &str) -> bool {
        self.streamers.contains_key(id)
    }

    /// Get a streamer actor handle.
    pub fn get_streamer(&self, id: &str) -> Option<&ActorHandle<StreamerMessage>> {
        self.streamers.get(id)
    }

    /// Get all streamer IDs.
    pub fn streamer_ids(&self) -> impl Iterator<Item = &String> {
        self.streamers.keys()
    }

    /// Get all streamer handles.
    pub fn streamer_handles(
        &self,
    ) -> impl Iterator<Item = (&String, &ActorHandle<StreamerMessage>)> {
        self.streamers.iter()
    }

    pub fn streamer_handles_map(&self) -> &HashMap<String, ActorHandle<StreamerMessage>> {
        &self.streamers
    }

    pub(crate) fn streamer_revision(&self) -> u64 {
        self.streamer_revision
    }

    /// Spawn a streamer actor and register it.
    ///
    /// Returns the actor handle if successful, or an error if an actor
    /// with the same ID already exists.
    pub fn spawn_streamer(
        &mut self,
        actor: StreamerActor,
        mut handle: ActorHandle<StreamerMessage>,
    ) -> Result<ActorHandle<StreamerMessage>, RegistryError> {
        let id = actor.id().to_string();

        if self.streamers.contains_key(&id) {
            return Err(RegistryError::ActorExists(id));
        }

        info!("Spawning streamer actor: {}", id);

        let generation = self.next_generation();
        handle.set_generation(generation);

        // Fires the handle's `ActorStopSignal` when the task below is dropped,
        // which is what `remove_streamer_awaitable` hands its caller.
        let stop_guard = handle.stop_signal().guard();

        // Clone handle for return
        let return_handle = handle.clone();

        // Store handle
        self.streamers.insert(id.clone(), handle);
        self.streamer_revision += 1;

        // Spawn actor task. `catch_unwind` turns a panic inside `run` into a
        // recoverable `ActorError` so `Supervisor::handle_task_completion` sees
        // a crash it can restart instead of a `JoinError` that carries no ID.
        self.task_set.spawn(async move {
            // Owned by the task so it outlives every exit path, including the
            // one an abort takes.
            let _stop_guard = stop_guard;
            let result = match AssertUnwindSafe(actor.run()).catch_unwind().await {
                Ok(result) => result,
                Err(payload) => Err(ActorError::recoverable(format!(
                    "Actor panicked: {}",
                    panic_payload_message(payload.as_ref())
                ))),
            };
            ActorTaskResult::streamer(id, generation, result)
        });

        Ok(return_handle)
    }

    /// Remove a streamer actor from the registry.
    ///
    /// This cancels the actor and removes its handle from the registry.
    /// The actor task will complete and be collected by `join_next`.
    ///
    /// Returns as soon as the handle is dropped. Callers that must know the
    /// actor's task has ended use [`Self::remove_streamer_awaitable`] instead;
    /// this method still records the actor's stop signal in
    /// `unreaped_streamers` so such a call can be made afterwards.
    pub fn remove_streamer(&mut self, id: &str) -> Option<ActorHandle<StreamerMessage>> {
        if let Some(handle) = self.streamers.remove(id) {
            self.streamer_revision += 1;
            debug!("Removing streamer actor: {}", id);
            handle.cancel();
            let entries = self.unreaped_streamers.entry(id.to_string()).or_default();
            entries.retain(|entry| !entry.stop_signal.is_stopped());
            entries.push(UnreapedActor {
                generation: handle.generation(),
                stop_signal: handle.stop_signal().clone(),
            });
            Some(handle)
        } else {
            None
        }
    }

    /// Remove a streamer actor and return a receipt that resolves once its task
    /// has actually left the runtime.
    ///
    /// Cancellation is the same as [`Self::remove_streamer`]: the handle is
    /// dropped and `ActorHandle::cancel` is fired, and an in-flight
    /// `StreamerActor::perform_check` keeps the task alive until it returns.
    /// The receipt closes that gap for callers that must not proceed while the
    /// actor can still write streamer state.
    ///
    /// The receipt tracks every task for `id` that is not known to have ended:
    /// the one this call removed, plus any that an earlier `remove_streamer`
    /// dropped the handle of and `handle_task_completion` has not reconciled
    /// yet. Each is identified by its registry generation, so a replacement
    /// spawned for the same `id` carries its own generation and its own stop
    /// signal and can neither satisfy nor extend a wait on an older one.
    ///
    /// [`ActorRemovalOutcome::NotRegistered`] therefore means no task for `id`
    /// is running, not merely that no handle was registered.
    pub fn remove_streamer_awaitable(&mut self, id: &str) -> ActorRemoval {
        let generation = self.remove_streamer(id).map(|handle| handle.generation());

        let tracked = match self.unreaped_streamers.get_mut(id) {
            Some(entries) => {
                entries.retain(|entry| !entry.stop_signal.is_stopped());
                if entries.is_empty() {
                    self.unreaped_streamers.remove(id);
                    Vec::new()
                } else {
                    entries.clone()
                }
            }
            None => Vec::new(),
        };

        ActorRemoval {
            actor_id: id.to_string(),
            generation,
            tracked,
        }
    }

    /// Wait for the next actor task to complete.
    ///
    /// Returns `None` if there are no more tasks.
    pub async fn join_next(&mut self) -> Option<Result<ActorTaskResult, tokio::task::JoinError>> {
        self.task_set.join_next().await
    }

    /// Check if there are any pending tasks.
    pub fn has_pending_tasks(&self) -> bool {
        !self.task_set.is_empty()
    }

    /// Get the number of pending tasks.
    pub fn pending_task_count(&self) -> usize {
        self.task_set.len()
    }

    /// Cancel all actors.
    pub fn cancel_all(&mut self) {
        info!("Cancelling all {} actors", self.streamer_count());

        for (id, handle) in &self.streamers {
            debug!("Cancelling streamer actor: {}", id);
            handle.cancel();
        }
    }

    /// Abort all actor tasks forcefully and wait for their futures to be dropped.
    pub async fn abort_all(&mut self) {
        warn!("Forcefully aborting all actor tasks");
        self.task_set.abort_all();

        while let Some(result) = self.task_set.join_next().await {
            match result {
                Ok(task_result) => {
                    debug!(
                        actor_id = %task_result.actor_id,
                        "Actor exited while forced shutdown was being applied"
                    );
                }
                Err(error) if error.is_cancelled() => {
                    debug!("Actor task reaped after forced shutdown");
                }
                Err(error) => {
                    warn!(%error, "Actor task failed while being reaped after forced shutdown");
                }
            }
        }
    }

    /// Clear all actors from the registry.
    ///
    /// This removes all handles but does not cancel or abort tasks.
    pub fn clear(&mut self) {
        if !self.streamers.is_empty() {
            self.streamer_revision += 1;
        }
        self.streamers.clear();
        // `Supervisor::shutdown` calls this after draining `task_set`, and
        // `abort_all` reaps tasks without going through `handle_task_completion`,
        // so nothing else would drop these entries.
        self.unreaped_streamers.clear();
    }

    /// Handle a completed actor task.
    ///
    /// Drops the actor's handle only when it still carries the generation the
    /// finished task was spawned under. `remove_streamer` cancels an actor but
    /// its task keeps running until the in-flight `check_status` returns, so by
    /// the time the old task reports its ID may hold a respawned actor, or
    /// nothing at all; in both cases the registered state wins and the result is
    /// flagged on `CompletedTask::superseded`.
    pub fn handle_task_completion(&mut self, result: ActorTaskResult) -> CompletedTask {
        self.forget_unreaped_streamer(&result.actor_id, result.generation);
        let superseded =
            remove_if_current(&mut self.streamers, &result.actor_id, result.generation);

        if superseded {
            debug!(
                actor_id = %result.actor_id,
                generation = result.generation,
                "Ignoring completion from an actor the registry no longer tracks"
            );
        } else {
            self.streamer_revision += 1;
        }

        CompletedTask { result, superseded }
    }

    /// Drop `unreaped_streamers`'s record of `generation`, and of any sibling
    /// entry whose signal has since fired, so the map tracks only tasks that may
    /// still be running.
    fn forget_unreaped_streamer(&mut self, actor_id: &str, generation: u64) {
        let Some(entries) = self.unreaped_streamers.get_mut(actor_id) else {
            return;
        };

        entries.retain(|entry| entry.generation != generation && !entry.stop_signal.is_stopped());
        if entries.is_empty() {
            self.unreaped_streamers.remove(actor_id);
        }
    }

    /// Get a child cancellation token for spawning actors.
    pub fn child_token(&self) -> CancellationToken {
        self.cancellation_token.child_token()
    }
}

/// Receipt for a removal performed by
/// [`ActorRegistry::remove_streamer_awaitable`].
///
/// Awaiting it does not depend on the scheduler event loop: the signal is fired
/// by the actor's own task as it is dropped, not by
/// `ActorRegistry::handle_task_completion`. A caller can therefore hold this
/// receipt across an await without stalling the loop that would otherwise have
/// to reap the task first.
#[derive(Debug, Clone)]
pub struct ActorRemoval {
    actor_id: String,
    generation: Option<u64>,
    tracked: Vec<UnreapedActor>,
}

impl ActorRemoval {
    /// The actor ID the removal was requested for.
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    /// The registry generation this call removed from the registry.
    ///
    /// `None` when no handle was registered under [`Self::actor_id`], which does
    /// not by itself mean nothing is running: an earlier `remove_streamer` can
    /// have dropped the handle of a task that is still inside `check_status`.
    /// [`Self::wait`] covers that task too.
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// The generations whose tasks [`Self::wait`] waits for, newest last.
    pub fn tracked_generations(&self) -> Vec<u64> {
        self.tracked.iter().map(|entry| entry.generation).collect()
    }

    /// Wait for every tracked actor task to end, giving up after `bound`.
    ///
    /// `bound` is the caller's: the registry imposes no timeout of its own, and
    /// it bounds the wait as a whole, not each tracked task.
    pub async fn wait(&self, bound: Duration) -> ActorRemovalOutcome {
        if self.tracked.is_empty() {
            return ActorRemovalOutcome::NotRegistered;
        }

        let stopped = async {
            for entry in &self.tracked {
                entry.stop_signal.stopped().await;
            }
        };

        match tokio::time::timeout(bound, stopped).await {
            Ok(()) => ActorRemovalOutcome::Stopped,
            Err(_) => ActorRemovalOutcome::TimedOut,
        }
    }
}

/// How an [`ActorRemoval`] wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorRemovalOutcome {
    /// No task for the ID was registered or still winding down, so there was
    /// nothing to stop.
    NotRegistered,
    /// Every task the receipt tracks left the runtime.
    Stopped,
    /// The caller's bound expired while at least one tracked task was still
    /// running.
    TimedOut,
}

/// A finished actor task reconciled against the registry by
/// `ActorRegistry::handle_task_completion`.
#[derive(Debug)]
pub struct CompletedTask {
    /// The task result as reported by the actor.
    pub result: ActorTaskResult,
    /// True when the registry no longer holds the handle this result belongs
    /// to: the actor was dropped by `remove_streamer`, or a
    /// newer spawn already took over its ID. Either way the result describes a
    /// task nobody is waiting on and it must not drive a restart.
    pub superseded: bool,
}

/// Drop `map`'s entry for `actor_id` when its handle carries `generation`.
///
/// Returns true when it does not: either a different generation is registered,
/// or no entry is left at all. Handles enter the map only through
/// `spawn_streamer`, so a missing entry means the actor was
/// deliberately dropped and its result is as stale as a replaced one.
fn remove_if_current<M>(
    map: &mut HashMap<String, ActorHandle<M>>,
    actor_id: &str,
    generation: u64,
) -> bool {
    if map.get(actor_id).map(ActorHandle::generation) == Some(generation) {
        map.remove(actor_id);
        return false;
    }

    true
}

/// Render a `catch_unwind` payload, covering the `&'static str` and `String`
/// shapes produced by `panic!`; anything else has no printable message.
fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Error type for registry operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// An actor with the given ID already exists.
    #[error("Actor already exists: {0}")]
    ActorExists(String),
    /// The actor was not found.
    #[error("Actor not found: {0}")]
    ActorNotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Priority, StreamerState};
    use crate::monitor::LiveStatus;
    use crate::scheduler::actor::messages::StreamerConfig;
    use crate::scheduler::actor::monitor_adapter::{CheckError, NoOpStatusChecker};
    use crate::streamer::StreamerMetadata;
    use async_trait::async_trait;
    use chrono::Utc;
    use dashmap::DashMap;
    use std::sync::Arc;
    use tokio::sync::Notify;

    /// How `BlockingStatusChecker::check_status` ends once the test releases it.
    #[derive(Clone, Copy)]
    enum OnRelease {
        Return,
        Panic,
    }

    /// Status checker whose `check_status` blocks until the test releases it,
    /// standing in for the in-flight check that keeps an actor's task alive after
    /// `ActorHandle::cancel`.
    struct BlockingStatusChecker {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        on_release: OnRelease,
    }

    impl BlockingStatusChecker {
        fn new(on_release: OnRelease) -> (Self, Arc<Notify>, Arc<Notify>) {
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            (
                Self {
                    entered: entered.clone(),
                    release: release.clone(),
                    on_release,
                },
                entered,
                release,
            )
        }
    }

    #[async_trait]
    impl super::super::monitor_adapter::StatusChecker for BlockingStatusChecker {
        async fn check_status(
            &self,
            _streamer: &StreamerMetadata,
        ) -> Result<(crate::scheduler::actor::messages::CheckResult, LiveStatus), CheckError>
        {
            // `notify_one` stores a permit when nobody is waiting yet, so neither
            // side of the handshake can miss the other.
            self.entered.notify_one();
            self.release.notified().await;
            if matches!(self.on_release, OnRelease::Panic) {
                std::panic::resume_unwind(Box::new("status check exploded"));
            }
            Ok((
                crate::scheduler::actor::messages::CheckResult::success(StreamerState::NotLive),
                LiveStatus::Offline,
            ))
        }

        async fn process_status(
            &self,
            _streamer: &StreamerMetadata,
            _status: LiveStatus,
        ) -> Result<crate::monitor::ProcessStatusResult, CheckError> {
            Ok(crate::monitor::ProcessStatusResult::Applied)
        }

        async fn handle_error(
            &self,
            _streamer: &StreamerMetadata,
            _error: &str,
        ) -> Result<(), CheckError> {
            Ok(())
        }

        async fn set_infra_blocked(
            &self,
            _streamer: &StreamerMetadata,
            _reason: crate::monitor::InfraBlockReason,
        ) -> Result<(), CheckError> {
            Ok(())
        }
    }

    fn create_test_metadata(id: &str) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: format!("Test Streamer {}", id),
            url: format!("https://twitch.tv/{}", id),
            platform_config_id: "twitch".to_string(),

            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
            last_error: None,
            streamer_specific_config: None,
            offline_check_count: 3,
            offline_check_delay_ms: 20_000,
            created_at: Utc::now(),
            deleted_at: None,
            updated_at: Utc::now(),
        }
    }

    fn create_test_metadata_store(id: &str) -> Arc<DashMap<String, Arc<StreamerMetadata>>> {
        let store = Arc::new(DashMap::new());
        let metadata = create_test_metadata(id);
        store.insert(id.to_string(), Arc::new(metadata));
        store
    }

    fn create_test_config() -> StreamerConfig {
        StreamerConfig {
            check_interval_ms: 1000,
            offline_check_interval_ms: 500,
            offline_check_count: 3,
            priority: Priority::Normal,
        }
    }

    fn create_noop_checker() -> Arc<dyn super::super::monitor_adapter::StatusChecker> {
        Arc::new(NoOpStatusChecker)
    }

    /// Spawn a streamer actor whose `check_status` blocks, and drive it into that
    /// check. Returns the registry handle plus the notifier that lets the check
    /// return.
    async fn spawn_blocked_in_check(
        registry: &mut ActorRegistry,
        token: &CancellationToken,
        id: &str,
        on_release: OnRelease,
    ) -> (ActorHandle<StreamerMessage>, Arc<Notify>) {
        let (checker, entered, release) = BlockingStatusChecker::new(on_release);
        let (actor, handle) = StreamerActor::new(
            id.to_string(),
            create_test_metadata_store(id),
            create_test_config(),
            token.child_token(),
            Arc::new(checker),
        );
        let handle = registry.spawn_streamer(actor, handle).unwrap();

        handle
            .send(StreamerMessage::CheckStatus)
            .await
            .expect("the actor should accept an immediate check");
        entered.notified().await;

        (handle, release)
    }

    #[tokio::test]
    async fn awaited_removal_resolves_once_the_in_flight_check_returns() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());
        let (handle, release) =
            spawn_blocked_in_check(&mut registry, &token, "test-1", OnRelease::Return).await;

        let removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(removal.generation(), Some(handle.generation()));
        assert_eq!(registry.streamer_count(), 0);

        // `remove_streamer_awaitable` fired `ActorHandle::cancel`, but the actor
        // is inside `check_status` and cannot observe it yet.
        assert_eq!(
            removal.wait(Duration::from_millis(50)).await,
            ActorRemovalOutcome::TimedOut
        );

        release.notify_one();
        assert_eq!(
            removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

        token.cancel();
    }

    #[tokio::test]
    async fn awaited_removal_resolves_when_the_actor_panics() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());
        let (_handle, release) =
            spawn_blocked_in_check(&mut registry, &token, "panics", OnRelease::Panic).await;

        let removal = registry.remove_streamer_awaitable("panics");
        release.notify_one();

        assert_eq!(
            removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

        // The task ended by unwinding out of the check that was in flight when
        // the removal was requested, not by observing the cancellation it fired.
        let result = registry
            .join_next()
            .await
            .expect("the actor task should finish")
            .expect("a panic must be caught inside the task");
        assert!(result.is_crash());
        assert!(
            result
                .error_message()
                .unwrap_or_default()
                .contains("status check exploded"),
            "unexpected error message: {:?}",
            result.error_message()
        );

        token.cancel();
    }

    #[tokio::test]
    async fn awaited_removal_is_not_satisfied_by_a_later_generation() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());
        let (first, release) =
            spawn_blocked_in_check(&mut registry, &token, "test-1", OnRelease::Return).await;

        let removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(removal.tracked_generations(), vec![first.generation()]);

        // A replacement takes the ID over while the first task is still inside
        // its check, and stops again.
        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            create_test_metadata_store("test-1"),
            create_test_config(),
            token.child_token(),
            create_noop_checker(),
        );
        let second = registry.spawn_streamer(actor, handle).unwrap();
        assert!(second.generation() > first.generation());

        // A receipt taken now covers both: the replacement it removes and the
        // first task, which is still running.
        let second_removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(second_removal.generation(), Some(second.generation()));
        assert_eq!(
            second_removal.tracked_generations(),
            vec![first.generation(), second.generation()]
        );

        // The replacement runs to completion and is reaped. Only it can finish:
        // the first task is still blocked in its check.
        let result = registry
            .join_next()
            .await
            .expect("the replacement task should finish")
            .expect("the task should not fail to join");
        assert_eq!(result.generation, second.generation());
        registry.handle_task_completion(result);

        // The replacement's exit says nothing about the generation the first
        // receipt tracks.
        assert_eq!(
            removal.wait(Duration::from_millis(50)).await,
            ActorRemovalOutcome::TimedOut
        );

        release.notify_one();
        assert_eq!(
            removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

        token.cancel();
    }

    #[tokio::test]
    async fn dropping_an_awaited_removal_leaves_nothing_behind() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            create_test_metadata_store("test-1"),
            create_test_config(),
            token.child_token(),
            create_noop_checker(),
        );
        let handle = registry.spawn_streamer(actor, handle).unwrap();

        let removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(removal.generation(), Some(handle.generation()));
        drop(removal);

        // The abandoned task is still reaped through the normal path.
        let result = registry
            .join_next()
            .await
            .expect("the actor task should finish")
            .expect("the task should not fail to join");
        assert!(registry.handle_task_completion(result).superseded);
        assert_eq!(registry.streamer_count(), 0);
        assert_eq!(registry.pending_task_count(), 0);

        // Reaping the generation cleared the registry's record of it, so a
        // second receipt has nothing left to wait on.
        let after_reap = registry.remove_streamer_awaitable("test-1");
        assert_eq!(after_reap.generation(), None);
        assert!(after_reap.tracked_generations().is_empty());
        assert_eq!(
            after_reap.wait(Duration::ZERO).await,
            ActorRemovalOutcome::NotRegistered
        );

        token.cancel();
    }

    /// `ensure_streamer_actor_state` and `handle_state_sync` reach for the
    /// non-awaiting `remove_streamer`, which drops the handle - and with it the
    /// only other copy of the stop signal - while the actor is still inside its
    /// check. A retirement asking for a receipt afterwards must not be told the
    /// actor is gone.
    #[tokio::test]
    async fn awaited_removal_covers_a_task_a_plain_removal_already_dropped() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());
        let (handle, release) =
            spawn_blocked_in_check(&mut registry, &token, "test-1", OnRelease::Return).await;

        assert!(registry.remove_streamer("test-1").is_some());

        let removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(removal.generation(), None);
        assert_eq!(removal.tracked_generations(), vec![handle.generation()]);
        assert_eq!(
            removal.wait(Duration::from_millis(50)).await,
            ActorRemovalOutcome::TimedOut
        );

        release.notify_one();
        assert_eq!(
            removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

        token.cancel();
    }
}
