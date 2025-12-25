//! Actor registry for tracking and managing actors.
//!
//! The `ActorRegistry` provides centralized management of actors:
//! - Tracks streamer and platform actors by ID
//! - Integrates with `JoinSet` for task management
//! - Supports actor spawning and removal
//! - Provides actor lookup and enumeration

use std::collections::HashMap;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::handle::ActorHandle;
use super::messages::{PlatformMessage, StreamerMessage};
use super::platform_actor::PlatformActor;
use super::streamer_actor::{ActorOutcome, ActorResult, StreamerActor};

/// Result of an actor task completion.
#[derive(Debug)]
pub struct ActorTaskResult {
    /// Actor ID.
    pub actor_id: String,
    /// Actor type ("streamer" or "platform").
    pub actor_type: String,
    /// The outcome of the actor's run.
    pub outcome: Result<ActorOutcome, String>,
}

impl ActorTaskResult {
    /// Create a result for a streamer actor.
    pub fn streamer(id: impl Into<String>, outcome: ActorResult) -> Self {
        Self {
            actor_id: id.into(),
            actor_type: "streamer".to_string(),
            outcome: outcome.map_err(|e| e.to_string()),
        }
    }

    /// Create a result for a platform actor.
    pub fn platform(id: impl Into<String>, outcome: ActorResult) -> Self {
        Self {
            actor_id: id.into(),
            actor_type: "platform".to_string(),
            outcome: outcome.map_err(|e| e.to_string()),
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
            Err(e) => Some(e.as_str()),
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
}

impl ActorRegistry {
    /// Create a new empty registry.
    pub fn new(cancellation_token: CancellationToken) -> Self {
        Self {
            streamers: HashMap::new(),
            platforms: HashMap::new(),
            task_set: JoinSet::new(),
            cancellation_token,
        }
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

    /// Get all platform handles.
    pub fn platform_handles(
        &self,
    ) -> impl Iterator<Item = (&String, &ActorHandle<PlatformMessage>)> {
        self.platforms.iter()
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
        handle: ActorHandle<StreamerMessage>,
    ) -> Result<ActorHandle<StreamerMessage>, RegistryError> {
        let id = actor.id().to_string();

        if self.streamers.contains_key(&id) {
            return Err(RegistryError::ActorExists(id));
        }

        info!("Spawning streamer actor: {}", id);

        // Clone handle for return
        let return_handle = handle.clone();

        // Store handle
        self.streamers.insert(id.clone(), handle);

        // Spawn actor task
        self.task_set.spawn(async move {
            let result = actor.run().await;
            ActorTaskResult::streamer(id, result)
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
        handle: ActorHandle<PlatformMessage>,
    ) -> Result<ActorHandle<PlatformMessage>, RegistryError> {
        let platform_id = actor.platform_id().to_string();

        if self.platforms.contains_key(&platform_id) {
            return Err(RegistryError::ActorExists(platform_id));
        }

        info!("Spawning platform actor: {}", platform_id);

        // Clone handle for return
        let return_handle = handle.clone();

        // Store handle
        self.platforms.insert(platform_id.clone(), handle);

        // Spawn actor task
        self.task_set.spawn(async move {
            let result = actor.run().await;
            ActorTaskResult::platform(platform_id, result)
        });

        Ok(return_handle)
    }

    /// Remove a streamer actor from the registry.
    ///
    /// This cancels the actor and removes its handle from the registry.
    /// The actor task will complete and be collected by `join_next`.
    pub fn remove_streamer(&mut self, id: &str) -> Option<ActorHandle<StreamerMessage>> {
        if let Some(handle) = self.streamers.remove(id) {
            debug!("Removing streamer actor: {}", id);
            handle.cancel();
            Some(handle)
        } else {
            None
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

    /// Abort all actor tasks forcefully.
    pub fn abort_all(&mut self) {
        warn!("Forcefully aborting all actor tasks");
        self.task_set.abort_all();
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
    /// Removes the actor from the registry if it completed normally.
    /// Returns the task result for further processing (e.g., restart decision).
    pub fn handle_task_completion(&mut self, result: ActorTaskResult) -> ActorTaskResult {
        match result.actor_type.as_str() {
            "streamer" => {
                self.streamers.remove(&result.actor_id);
            }
            "platform" => {
                self.platforms.remove(&result.actor_id);
            }
            _ => {
                warn!("Unknown actor type: {}", result.actor_type);
            }
        }

        result
    }

    /// Get a child cancellation token for spawning actors.
    pub fn child_token(&self) -> CancellationToken {
        self.cancellation_token.child_token()
    }
}

/// Error type for registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// An actor with the given ID already exists.
    ActorExists(String),
    /// The actor was not found.
    ActorNotFound(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::ActorExists(id) => write!(f, "Actor already exists: {}", id),
            RegistryError::ActorNotFound(id) => write!(f, "Actor not found: {}", id),
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Priority, StreamerState};
    use crate::scheduler::actor::messages::StreamerConfig;
    use crate::scheduler::actor::monitor_adapter::NoOpStatusChecker;
    use crate::streamer::StreamerMetadata;
    use chrono::Utc;
    use dashmap::DashMap;
    use std::sync::Arc;

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
        let stopped = ActorTaskResult::streamer("test", Ok(ActorOutcome::Stopped));
        assert!(!stopped.is_crash());

        let cancelled = ActorTaskResult::streamer("test", Ok(ActorOutcome::Cancelled));
        assert!(!cancelled.is_crash());

        let error = ActorTaskResult::streamer(
            "test",
            Err(crate::scheduler::actor::streamer_actor::ActorError::fatal(
                "test error",
            )),
        );
        assert!(error.is_crash());
        assert_eq!(error.error_message(), Some("test error"));
    }

    #[test]
    fn test_registry_error_display() {
        let exists = RegistryError::ActorExists("test".to_string());
        assert_eq!(exists.to_string(), "Actor already exists: test");

        let not_found = RegistryError::ActorNotFound("test".to_string());
        assert_eq!(not_found.to_string(), "Actor not found: test");
    }
}
