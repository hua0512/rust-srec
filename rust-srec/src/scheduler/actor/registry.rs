//! Actor registry for tracking and managing actors.
//!
//! The `ActorRegistry` provides centralized management of actors:
//! - Tracks streamer and platform actors by ID
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
use super::messages::{PlatformMessage, StreamerMessage};
use super::platform_actor::PlatformActor;
use super::streamer_actor::{ActorError, ActorOutcome, ActorResult, StreamerActor};

/// Result of an actor task completion.
#[derive(Debug)]
pub struct ActorTaskResult {
    /// Actor ID.
    pub actor_id: String,
    /// Actor type ("streamer" or "platform").
    pub actor_type: String,
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
            actor_type: "streamer".to_string(),
            generation,
            outcome,
        }
    }

    /// Create a result for a platform actor.
    pub fn platform(id: impl Into<String>, generation: u64, outcome: ActorResult) -> Self {
        Self {
            actor_id: id.into(),
            actor_type: "platform".to_string(),
            generation,
            outcome,
        }
    }

    /// Check if the actor crashed (error or unexpected outcome).
    pub fn is_crash(&self) -> bool {
        match &self.outcome {
            Ok(ActorOutcome::Stopped) | Ok(ActorOutcome::Cancelled) => false,
            Ok(ActorOutcome::Completed) => false,
            Err(_) => true,
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
    /// Platform actors by platform ID.
    platforms: HashMap<String, ActorHandle<PlatformMessage>>,
    /// JoinSet for actor tasks.
    task_set: JoinSet<ActorTaskResult>,
    /// Parent cancellation token.
    cancellation_token: CancellationToken,
    /// Source of the generation stamped onto every spawned actor. Starts at 0
    /// so `next_generation` never hands out the "unregistered" value 0.
    generation_counter: u64,
}

impl ActorRegistry {
    /// Create a new empty registry.
    pub fn new(cancellation_token: CancellationToken) -> Self {
        Self {
            streamers: HashMap::new(),
            platforms: HashMap::new(),
            task_set: JoinSet::new(),
            cancellation_token,
            generation_counter: 0,
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

    /// Get the number of platform actors.
    pub fn platform_count(&self) -> usize {
        self.platforms.len()
    }

    /// Get the total number of actors.
    pub fn total_count(&self) -> usize {
        self.streamers.len() + self.platforms.len()
    }

    /// Check if a streamer actor exists.
    pub fn has_streamer(&self, id: &str) -> bool {
        self.streamers.contains_key(id)
    }

    /// Check if a platform actor exists.
    pub fn has_platform(&self, platform_id: &str) -> bool {
        self.platforms.contains_key(platform_id)
    }

    /// Get a streamer actor handle.
    pub fn get_streamer(&self, id: &str) -> Option<&ActorHandle<StreamerMessage>> {
        self.streamers.get(id)
    }

    /// Get a platform actor handle.
    pub fn get_platform(&self, platform_id: &str) -> Option<&ActorHandle<PlatformMessage>> {
        self.platforms.get(platform_id)
    }

    /// Get all streamer IDs.
    pub fn streamer_ids(&self) -> impl Iterator<Item = &String> {
        self.streamers.keys()
    }

    /// Get all platform IDs.
    pub fn platform_ids(&self) -> impl Iterator<Item = &String> {
        self.platforms.keys()
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

    /// Get all platform handles.
    pub fn platform_handles(
        &self,
    ) -> impl Iterator<Item = (&String, &ActorHandle<PlatformMessage>)> {
        self.platforms.iter()
    }

    pub fn platform_handles_map(&self) -> &HashMap<String, ActorHandle<PlatformMessage>> {
        &self.platforms
    }

    /// Get streamers on a specific platform.
    pub fn streamers_on_platform(&self, platform_id: &str) -> Vec<&ActorHandle<StreamerMessage>> {
        self.streamers
            .values()
            .filter(|h| h.metadata.id.contains(platform_id))
            .collect()
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

    /// Spawn a platform actor and register it.
    ///
    /// Returns the actor handle if successful, or an error if an actor
    /// with the same platform ID already exists.
    pub fn spawn_platform(
        &mut self,
        actor: PlatformActor,
        mut handle: ActorHandle<PlatformMessage>,
    ) -> Result<ActorHandle<PlatformMessage>, RegistryError> {
        let platform_id = actor.platform_id().to_string();

        if self.platforms.contains_key(&platform_id) {
            return Err(RegistryError::ActorExists(platform_id));
        }

        info!("Spawning platform actor: {}", platform_id);

        let generation = self.next_generation();
        handle.set_generation(generation);

        // Fires the handle's `ActorStopSignal` when the task below is dropped,
        // mirroring `spawn_streamer`.
        let stop_guard = handle.stop_signal().guard();

        // Clone handle for return
        let return_handle = handle.clone();

        // Store handle
        self.platforms.insert(platform_id.clone(), handle);

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
            ActorTaskResult::platform(platform_id, generation, result)
        });

        Ok(return_handle)
    }

    /// Remove a streamer actor from the registry.
    ///
    /// This cancels the actor and removes its handle from the registry.
    /// The actor task will complete and be collected by `join_next`.
    ///
    /// Returns as soon as the handle is dropped. Callers that must know the
    /// actor's task has ended use [`Self::remove_streamer_awaitable`] instead.
    pub fn remove_streamer(&mut self, id: &str) -> Option<ActorHandle<StreamerMessage>> {
        if let Some(handle) = self.streamers.remove(id) {
            debug!("Removing streamer actor: {}", id);
            handle.cancel();
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
    /// The receipt tracks the exact task that was registered under `id` at this
    /// call, identified by its registry generation. A replacement spawned for
    /// the same `id` carries its own generation and its own
    /// `ActorHandle::stop_signal`, so it can neither satisfy nor extend this
    /// wait. Nothing is stored in the registry: the receipt owns the only extra
    /// state, and dropping it unregistered leaves the actor task untouched.
    pub fn remove_streamer_awaitable(&mut self, id: &str) -> ActorRemoval {
        match self.remove_streamer(id) {
            Some(handle) => ActorRemoval {
                actor_id: id.to_string(),
                generation: Some(handle.generation()),
                stop_signal: handle.stop_signal().clone(),
            },
            None => ActorRemoval {
                actor_id: id.to_string(),
                generation: None,
                stop_signal: ActorStopSignal::already_stopped(),
            },
        }
    }

    /// Remove a platform actor from the registry.
    ///
    /// This cancels the actor and removes its handle from the registry.
    pub fn remove_platform(&mut self, platform_id: &str) -> Option<ActorHandle<PlatformMessage>> {
        if let Some(handle) = self.platforms.remove(platform_id) {
            debug!("Removing platform actor: {}", platform_id);
            handle.cancel();
            Some(handle)
        } else {
            None
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
        info!("Cancelling all {} actors", self.total_count());

        for (id, handle) in &self.streamers {
            debug!("Cancelling streamer actor: {}", id);
            handle.cancel();
        }

        for (id, handle) in &self.platforms {
            debug!("Cancelling platform actor: {}", id);
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
                        actor_type = %task_result.actor_type,
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
        self.streamers.clear();
        self.platforms.clear();
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
        let superseded = match result.actor_type.as_str() {
            "streamer" => {
                remove_if_current(&mut self.streamers, &result.actor_id, result.generation)
            }
            "platform" => {
                remove_if_current(&mut self.platforms, &result.actor_id, result.generation)
            }
            _ => {
                warn!("Unknown actor type: {}", result.actor_type);
                false
            }
        };

        if superseded {
            debug!(
                actor_id = %result.actor_id,
                actor_type = %result.actor_type,
                generation = result.generation,
                "Ignoring completion from an actor the registry no longer tracks"
            );
        }

        CompletedTask { result, superseded }
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
    stop_signal: ActorStopSignal,
}

impl ActorRemoval {
    /// The actor ID the removal was requested for.
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    /// The registry generation whose task this receipt tracks.
    ///
    /// `None` when no actor was registered under [`Self::actor_id`], in which
    /// case [`Self::wait`] resolves to [`ActorRemovalOutcome::NotRegistered`]
    /// without awaiting anything.
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// Wait for the removed actor's task to end, giving up after `bound`.
    ///
    /// `bound` is the caller's: the registry imposes no timeout of its own.
    pub async fn wait(&self, bound: Duration) -> ActorRemovalOutcome {
        if self.generation.is_none() {
            return ActorRemovalOutcome::NotRegistered;
        }

        match tokio::time::timeout(bound, self.stop_signal.stopped()).await {
            Ok(()) => ActorRemovalOutcome::Stopped,
            Err(_) => ActorRemovalOutcome::TimedOut,
        }
    }
}

/// How an [`ActorRemoval`] wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorRemovalOutcome {
    /// No actor was registered under the ID, so there was nothing to stop.
    NotRegistered,
    /// The actor's task left the runtime.
    Stopped,
    /// The caller's bound expired while the task was still running.
    TimedOut,
}

impl ActorRemovalOutcome {
    /// Whether the actor is known not to be running any more.
    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::NotRegistered | Self::Stopped)
    }
}

/// A finished actor task reconciled against the registry by
/// `ActorRegistry::handle_task_completion`.
#[derive(Debug)]
pub struct CompletedTask {
    /// The task result as reported by the actor.
    pub result: ActorTaskResult,
    /// True when the registry no longer holds the handle this result belongs
    /// to: the actor was dropped by `remove_streamer` / `remove_platform`, or a
    /// newer spawn already took over its ID. Either way the result describes a
    /// task nobody is waiting on and it must not drive a restart.
    pub superseded: bool,
}

/// Drop `map`'s entry for `actor_id` when its handle carries `generation`.
///
/// Returns true when it does not: either a different generation is registered,
/// or no entry is left at all. Handles enter the map only through
/// `spawn_streamer` / `spawn_platform`, so a missing entry means the actor was
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
                panic!("status check exploded");
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

    /// Status checker that unwinds inside `StreamerActor::perform_check`.
    struct PanickingStatusChecker;

    #[async_trait]
    impl super::super::monitor_adapter::StatusChecker for PanickingStatusChecker {
        async fn check_status(
            &self,
            _streamer: &StreamerMetadata,
        ) -> Result<(crate::scheduler::actor::messages::CheckResult, LiveStatus), CheckError>
        {
            panic!("status check exploded");
        }

        async fn process_status(
            &self,
            _streamer: &StreamerMetadata,
            _status: LiveStatus,
        ) -> Result<crate::monitor::ProcessStatusResult, CheckError> {
            unreachable!("the panicking check never produces a status")
        }

        async fn handle_error(
            &self,
            _streamer: &StreamerMetadata,
            _error: &str,
        ) -> Result<(), CheckError> {
            unreachable!("the panicking check never produces an error")
        }

        async fn set_infra_blocked(
            &self,
            _streamer: &StreamerMetadata,
            _reason: crate::monitor::InfraBlockReason,
        ) -> Result<(), CheckError> {
            unreachable!("the panicking check never applies an infrastructure block")
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
            updated_at: Utc::now(),
        }
    }

    fn create_test_metadata_store(id: &str) -> Arc<DashMap<String, StreamerMetadata>> {
        let store = Arc::new(DashMap::new());
        let metadata = create_test_metadata(id);
        store.insert(id.to_string(), metadata);
        store
    }

    fn create_test_config() -> StreamerConfig {
        StreamerConfig {
            check_interval_ms: 1000,
            offline_check_interval_ms: 500,
            offline_check_count: 3,
            priority: Priority::Normal,
            batch_capable: false,
        }
    }

    fn create_noop_checker() -> Arc<dyn super::super::monitor_adapter::StatusChecker> {
        Arc::new(NoOpStatusChecker)
    }

    #[test]
    fn test_registry_new() {
        let token = CancellationToken::new();
        let registry = ActorRegistry::new(token);

        assert_eq!(registry.streamer_count(), 0);
        assert_eq!(registry.platform_count(), 0);
        assert_eq!(registry.total_count(), 0);
    }

    #[tokio::test]
    async fn test_registry_spawn_streamer() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let metadata_store = create_test_metadata_store("test-1");
        let config = create_test_config();
        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store,
            config,
            token.child_token(),
            create_noop_checker(),
        );

        let result = registry.spawn_streamer(actor, handle);
        assert!(result.is_ok());
        assert_eq!(registry.streamer_count(), 1);
        assert!(registry.has_streamer("test-1"));

        // Cancel to clean up
        token.cancel();
    }

    #[tokio::test]
    async fn test_registry_spawn_duplicate_streamer() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let metadata_store = create_test_metadata_store("test-1");
        let config = create_test_config();

        // Spawn first actor
        let (actor1, handle1) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store.clone(),
            config.clone(),
            token.child_token(),
            create_noop_checker(),
        );
        registry.spawn_streamer(actor1, handle1).unwrap();

        // Try to spawn duplicate
        let (actor2, handle2) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store,
            config,
            token.child_token(),
            create_noop_checker(),
        );
        let result = registry.spawn_streamer(actor2, handle2);

        assert!(matches!(result, Err(RegistryError::ActorExists(_))));
        assert_eq!(registry.streamer_count(), 1);

        // Cancel to clean up
        token.cancel();
    }

    #[tokio::test]
    async fn test_registry_remove_streamer() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let metadata_store = create_test_metadata_store("test-1");
        let config = create_test_config();
        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store,
            config,
            token.child_token(),
            create_noop_checker(),
        );

        registry.spawn_streamer(actor, handle).unwrap();
        assert_eq!(registry.streamer_count(), 1);

        let removed = registry.remove_streamer("test-1");
        assert!(removed.is_some());
        assert_eq!(registry.streamer_count(), 0);
        assert!(!registry.has_streamer("test-1"));

        // Cancel to clean up
        token.cancel();
    }

    #[tokio::test]
    async fn test_registry_cancel_all() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        // Create a shared metadata store for all actors
        let metadata_store = Arc::new(DashMap::new());

        // Spawn multiple actors
        for i in 0..3 {
            let id = format!("test-{}", i);
            let metadata = create_test_metadata(&id);
            metadata_store.insert(id.clone(), metadata);

            let config = create_test_config();
            let (actor, handle) = StreamerActor::new(
                id,
                metadata_store.clone(),
                config,
                token.child_token(),
                create_noop_checker(),
            );
            registry.spawn_streamer(actor, handle).unwrap();
        }

        assert_eq!(registry.streamer_count(), 3);

        // Cancel all
        registry.cancel_all();

        // All handles should be cancelled
        for (_, handle) in registry.streamer_handles() {
            assert!(handle.is_cancelled());
        }
    }

    #[test]
    fn test_actor_task_result_is_crash() {
        let stopped = ActorTaskResult::streamer("test", 1, Ok(ActorOutcome::Stopped));
        assert!(!stopped.is_crash());

        let cancelled = ActorTaskResult::streamer("test", 1, Ok(ActorOutcome::Cancelled));
        assert!(!cancelled.is_crash());

        let error = ActorTaskResult::streamer(
            "test",
            1,
            Err(crate::scheduler::actor::streamer_actor::ActorError::fatal(
                "test error",
            )),
        );
        assert!(error.is_crash());
        assert_eq!(error.error_message(), Some("test error"));
    }

    #[tokio::test]
    async fn panicking_actor_run_is_reported_as_a_crash() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let (actor, handle) = StreamerActor::with_priority_channel(
            "panics".to_string(),
            create_test_metadata_store("panics"),
            create_test_config(),
            token.child_token(),
            Arc::new(PanickingStatusChecker),
        );
        let handle = registry.spawn_streamer(actor, handle).unwrap();

        handle
            .send(StreamerMessage::CheckStatus)
            .await
            .expect("the actor should accept an immediate check");

        let joined = registry
            .join_next()
            .await
            .expect("the actor task should finish");
        let result =
            joined.expect("a panic must be caught inside the task, not become a JoinError");

        assert_eq!(result.generation, handle.generation());
        assert!(result.is_crash());
        assert!(
            result
                .error_message()
                .unwrap_or_default()
                .contains("status check exploded"),
            "the panic payload should reach the restart path: {:?}",
            result.error_message()
        );

        token.cancel();
    }

    #[tokio::test]
    async fn stale_completion_keeps_the_replacement_handle() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());
        let metadata_store = create_test_metadata_store("test-1");

        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store.clone(),
            create_test_config(),
            token.child_token(),
            create_noop_checker(),
        );
        let first = registry.spawn_streamer(actor, handle).unwrap();

        // `remove_streamer` only cancels; the first task can still be running.
        registry.remove_streamer("test-1");

        let (actor, handle) = StreamerActor::new(
            "test-1".to_string(),
            metadata_store,
            create_test_config(),
            token.child_token(),
            create_noop_checker(),
        );
        let second = registry.spawn_streamer(actor, handle).unwrap();
        assert!(second.generation() > first.generation());

        let completed = registry.handle_task_completion(ActorTaskResult::streamer(
            "test-1",
            first.generation(),
            Ok(ActorOutcome::Cancelled),
        ));

        assert!(completed.superseded);
        assert_eq!(
            registry.get_streamer("test-1").map(|h| h.generation()),
            Some(second.generation())
        );

        // The replacement's own completion still clears the entry.
        let completed = registry.handle_task_completion(ActorTaskResult::streamer(
            "test-1",
            second.generation(),
            Ok(ActorOutcome::Stopped),
        ));

        assert!(!completed.superseded);
        assert!(!registry.has_streamer("test-1"));

        // Once the entry is gone there is nothing left to reconcile against, so
        // a repeat of the same completion is stale too.
        let completed = registry.handle_task_completion(ActorTaskResult::streamer(
            "test-1",
            second.generation(),
            Ok(ActorOutcome::Stopped),
        ));

        assert!(completed.superseded);

        token.cancel();
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
    async fn awaited_removal_of_an_unregistered_streamer_needs_no_wait() {
        let token = CancellationToken::new();
        let mut registry = ActorRegistry::new(token.clone());

        let removal = registry.remove_streamer_awaitable("never-spawned");

        assert_eq!(removal.actor_id(), "never-spawned");
        assert_eq!(removal.generation(), None);
        assert_eq!(
            removal.wait(Duration::ZERO).await,
            ActorRemovalOutcome::NotRegistered
        );
        assert!(removal.wait(Duration::ZERO).await.is_stopped());
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

        let second_removal = registry.remove_streamer_awaitable("test-1");
        assert_eq!(second_removal.generation(), Some(second.generation()));
        assert_eq!(
            second_removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

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

        // Nothing is registered for the ID any more, so a second receipt has
        // nothing to wait on.
        let after_drop = registry.remove_streamer_awaitable("test-1");
        assert_eq!(after_drop.generation(), None);
        assert_eq!(
            after_drop.wait(Duration::ZERO).await,
            ActorRemovalOutcome::NotRegistered
        );

        // The abandoned task is still reaped through the normal path.
        let result = registry
            .join_next()
            .await
            .expect("the actor task should finish")
            .expect("the task should not fail to join");
        assert!(registry.handle_task_completion(result).superseded);
        assert_eq!(registry.streamer_count(), 0);
        assert_eq!(registry.pending_task_count(), 0);

        token.cancel();
    }

    #[test]
    fn test_registry_error_display() {
        let exists = RegistryError::ActorExists("test".to_string());
        assert_eq!(exists.to_string(), "Actor already exists: test");

        let not_found = RegistryError::ActorNotFound("test".to_string());
        assert_eq!(not_found.to_string(), "Actor not found: test");
    }
}
