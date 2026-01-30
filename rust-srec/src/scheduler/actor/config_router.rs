//! Configuration router for targeted delivery of config updates.
//!
//! The `ConfigRouter` routes configuration update events to the appropriate actors:
//! - Streamer-specific updates go to a single StreamerActor
//! - Platform updates go to all StreamerActors on that platform
//! - Global updates go to all actors
//!
//! This implements the scheduler-actor-refactoring spec.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use super::handle::ActorHandle;
use super::messages::{PlatformConfig, PlatformMessage, StreamerConfig, StreamerMessage};
use crate::config::events::ConfigUpdateEvent;

/// Result of a routing operation.
#[derive(Debug, Clone)]
pub struct RoutingResult {
    /// Number of actors that received the update.
    pub delivered: usize,
    /// Number of actors that failed to receive the update.
    pub failed: usize,
    /// IDs of actors that failed to receive the update.
    pub failed_actors: Vec<String>,
}

impl RoutingResult {
    /// Create a new empty result.
    pub fn new() -> Self {
        Self {
            delivered: 0,
            failed: 0,
            failed_actors: Vec::new(),
        }
    }

    /// Record a successful delivery.
    pub fn record_success(&mut self) {
        self.delivered += 1;
    }

    /// Record a failed delivery.
    pub fn record_failure(&mut self, actor_id: impl Into<String>) {
        self.failed += 1;
        self.failed_actors.push(actor_id.into());
    }

    /// Check if all deliveries succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0
    }

    /// Get the total number of attempted deliveries.
    pub fn total(&self) -> usize {
        self.delivered + self.failed
    }
}

impl Default for RoutingResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Scope of a configuration update for routing purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigScope {
    /// Update applies to a single streamer.
    Streamer(String),
    /// Update applies to all streamers on a platform.
    Platform(String),
    /// Update applies to all actors globally.
    Global,
}

impl ConfigScope {
    /// Create a scope from a ConfigUpdateEvent.
    pub fn from_event(event: &ConfigUpdateEvent) -> Self {
        match event {
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                ConfigScope::Streamer(streamer_id.clone())
            }
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                ConfigScope::Streamer(streamer_id.clone())
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, .. } => {
                ConfigScope::Streamer(streamer_id.clone())
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                ConfigScope::Streamer(streamer_id.clone())
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                ConfigScope::Platform(platform_id.clone())
            }
            ConfigUpdateEvent::GlobalUpdated => ConfigScope::Global,
            // Template and engine updates are treated as global for now
            ConfigUpdateEvent::TemplateUpdated { .. } => ConfigScope::Global,
            ConfigUpdateEvent::EngineUpdated { .. } => ConfigScope::Global,
        }
    }
}

/// Mapping of streamer IDs to their platform IDs.
///
/// This is used to determine which streamers belong to which platform
/// for platform-scoped config updates.
pub struct PlatformMapping {
    /// Map from streamer ID to platform ID.
    streamer_to_platform: HashMap<String, String>,
}

impl PlatformMapping {
    /// Create a new empty mapping.
    pub fn new() -> Self {
        Self {
            streamer_to_platform: HashMap::new(),
        }
    }

    /// Register a streamer's platform association.
    pub fn register(&mut self, streamer_id: impl Into<String>, platform_id: impl Into<String>) {
        self.streamer_to_platform
            .insert(streamer_id.into(), platform_id.into());
    }

    /// Remove a streamer's platform association.
    pub fn unregister(&mut self, streamer_id: &str) {
        self.streamer_to_platform.remove(streamer_id);
    }

    /// Get the platform ID for a streamer.
    pub fn get_platform(&self, streamer_id: &str) -> Option<&String> {
        self.streamer_to_platform.get(streamer_id)
    }

    /// Get all streamer IDs on a specific platform.
    pub fn streamers_on_platform(&self, platform_id: &str) -> Vec<&String> {
        self.streamer_to_platform
            .iter()
            .filter(|(_, p)| *p == platform_id)
            .map(|(s, _)| s)
            .collect()
    }

    /// Get the number of registered streamers.
    pub fn len(&self) -> usize {
        self.streamer_to_platform.len()
    }

    /// Check if the mapping is empty.
    pub fn is_empty(&self) -> bool {
        self.streamer_to_platform.is_empty()
    }
}

impl Default for PlatformMapping {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration router for targeted delivery of config updates.
///
/// The router maintains references to actor handles and a platform mapping
/// to route configuration updates to the appropriate actors based on scope.
pub struct ConfigRouter<'a> {
    /// Streamer actor handles by ID.
    streamer_handles: &'a HashMap<String, ActorHandle<StreamerMessage>>,
    /// Platform actor handles by platform ID.
    platform_handles: &'a HashMap<String, ActorHandle<PlatformMessage>>,
    /// Platform mapping for streamer-to-platform associations.
    platform_mapping: &'a PlatformMapping,
}

impl<'a> ConfigRouter<'a> {
    /// Create a new config router.
    pub fn new(
        streamer_handles: &'a HashMap<String, ActorHandle<StreamerMessage>>,
        platform_handles: &'a HashMap<String, ActorHandle<PlatformMessage>>,
        platform_mapping: &'a PlatformMapping,
    ) -> Self {
        Self {
            streamer_handles,
            platform_handles,
            platform_mapping,
        }
    }

    /// Route a configuration update event to the appropriate actors.
    ///
    /// Returns a result indicating how many actors received the update.
    pub async fn route_event(
        &self,
        event: &ConfigUpdateEvent,
        streamer_config_fn: impl Fn(&str) -> StreamerConfig,
        platform_config_fn: impl Fn(&str) -> PlatformConfig,
    ) -> RoutingResult {
        let scope = ConfigScope::from_event(event);
        self.route_with_scope(&scope, streamer_config_fn, platform_config_fn)
            .await
    }

    /// Route a configuration update with a specific scope.
    pub async fn route_with_scope(
        &self,
        scope: &ConfigScope,
        streamer_config_fn: impl Fn(&str) -> StreamerConfig,
        platform_config_fn: impl Fn(&str) -> PlatformConfig,
    ) -> RoutingResult {
        match scope {
            ConfigScope::Streamer(streamer_id) => {
                self.route_to_streamer(streamer_id, &streamer_config_fn)
                    .await
            }
            ConfigScope::Platform(platform_id) => {
                self.route_to_platform(platform_id, &streamer_config_fn, &platform_config_fn)
                    .await
            }
            ConfigScope::Global => {
                self.route_globally(&streamer_config_fn, &platform_config_fn)
                    .await
            }
        }
    }

    /// Route a config update to a single streamer actor.
    ///
    /// Implements Requirement 3.1: Streamer-specific updates go to single actor.
    async fn route_to_streamer(
        &self,
        streamer_id: &str,
        config_fn: &impl Fn(&str) -> StreamerConfig,
    ) -> RoutingResult {
        let mut result = RoutingResult::new();

        if let Some(handle) = self.streamer_handles.get(streamer_id) {
            let config = config_fn(streamer_id);
            match handle.send(StreamerMessage::ConfigUpdate(config)).await {
                Ok(()) => {
                    debug!("Routed config update to streamer {}", streamer_id);
                    result.record_success();
                }
                Err(e) => {
                    warn!(
                        "Failed to route config update to streamer {}: {}",
                        streamer_id, e
                    );
                    result.record_failure(streamer_id);
                }
            }
        } else {
            debug!("Streamer {} not found for config routing", streamer_id);
        }

        result
    }

    /// Route a config update to all actors on a platform.
    ///
    /// Implements Requirement 3.2: Platform updates go to all actors on platform.
    async fn route_to_platform(
        &self,
        platform_id: &str,
        streamer_config_fn: &impl Fn(&str) -> StreamerConfig,
        platform_config_fn: &impl Fn(&str) -> PlatformConfig,
    ) -> RoutingResult {
        let mut result = RoutingResult::new();

        // Route to platform actor if it exists
        if let Some(handle) = self.platform_handles.get(platform_id) {
            let config = platform_config_fn(platform_id);
            match handle.send(PlatformMessage::ConfigUpdate(config)).await {
                Ok(()) => {
                    debug!("Routed config update to platform actor {}", platform_id);
                    result.record_success();
                }
                Err(e) => {
                    warn!(
                        "Failed to route config update to platform actor {}: {}",
                        platform_id, e
                    );
                    result.record_failure(format!("platform:{}", platform_id));
                }
            }
        }

        // Route to all streamers on this platform
        let streamers = self.platform_mapping.streamers_on_platform(platform_id);
        for streamer_id in streamers {
            if let Some(handle) = self.streamer_handles.get(streamer_id) {
                let config = streamer_config_fn(streamer_id);
                match handle.send(StreamerMessage::ConfigUpdate(config)).await {
                    Ok(()) => {
                        debug!(
                            "Routed platform config update to streamer {} on {}",
                            streamer_id, platform_id
                        );
                        result.record_success();
                    }
                    Err(e) => {
                        warn!(
                            "Failed to route platform config update to streamer {}: {}",
                            streamer_id, e
                        );
                        result.record_failure(streamer_id);
                    }
                }
            }
        }

        info!(
            "Routed platform {} config update: {} delivered, {} failed",
            platform_id, result.delivered, result.failed
        );

        result
    }

    /// Route a config update to all actors globally.
    ///
    /// Implements Requirement 3.3: Global updates go to all actors.
    async fn route_globally(
        &self,
        streamer_config_fn: &impl Fn(&str) -> StreamerConfig,
        platform_config_fn: &impl Fn(&str) -> PlatformConfig,
    ) -> RoutingResult {
        let mut result = RoutingResult::new();

        // Route to all platform actors
        for (platform_id, handle) in self.platform_handles.iter() {
            let config = platform_config_fn(platform_id);
            match handle.send(PlatformMessage::ConfigUpdate(config)).await {
                Ok(()) => {
                    debug!(
                        "Routed global config update to platform actor {}",
                        platform_id
                    );
                    result.record_success();
                }
                Err(e) => {
                    warn!(
                        "Failed to route global config update to platform actor {}: {}",
                        platform_id, e
                    );
                    result.record_failure(format!("platform:{}", platform_id));
                }
            }
        }

        // Route to all streamer actors
        for (streamer_id, handle) in self.streamer_handles.iter() {
            let config = streamer_config_fn(streamer_id);
            match handle.send(StreamerMessage::ConfigUpdate(config)).await {
                Ok(()) => {
                    debug!("Routed global config update to streamer {}", streamer_id);
                    result.record_success();
                }
                Err(e) => {
                    warn!(
                        "Failed to route global config update to streamer {}: {}",
                        streamer_id, e
                    );
                    result.record_failure(streamer_id);
                }
            }
        }

        info!(
            "Routed global config update: {} delivered, {} failed",
            result.delivered, result.failed
        );

        result
    }

    /// Get the actors that would receive an update for a given scope.
    ///
    /// This is useful for testing and debugging to verify routing logic
    /// without actually sending messages.
    pub fn get_target_actors(&self, scope: &ConfigScope) -> Vec<String> {
        match scope {
            ConfigScope::Streamer(streamer_id) => {
                if self.streamer_handles.contains_key(streamer_id) {
                    vec![streamer_id.clone()]
                } else {
                    vec![]
                }
            }
            ConfigScope::Platform(platform_id) => {
                let mut targets = Vec::new();

                // Add platform actor if exists
                if self.platform_handles.contains_key(platform_id) {
                    targets.push(format!("platform:{}", platform_id));
                }

                // Add all streamers on this platform
                for streamer_id in self.platform_mapping.streamers_on_platform(platform_id) {
                    targets.push(streamer_id.clone());
                }

                targets
            }
            ConfigScope::Global => {
                let mut targets = Vec::new();

                // Add all platform actors
                for platform_id in self.platform_handles.keys() {
                    targets.push(format!("platform:{}", platform_id));
                }

                // Add all streamer actors
                for streamer_id in self.streamer_handles.keys() {
                    targets.push(streamer_id.clone());
                }

                targets
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::actor::handle::ActorMetadata;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn create_test_streamer_handle(
        id: &str,
    ) -> (
        ActorHandle<StreamerMessage>,
        mpsc::Receiver<StreamerMessage>,
    ) {
        let (tx, rx) = mpsc::channel(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer(id, false);
        let handle = ActorHandle::new(tx, token, metadata);
        (handle, rx)
    }

    fn create_test_platform_handle(
        id: &str,
    ) -> (
        ActorHandle<PlatformMessage>,
        mpsc::Receiver<PlatformMessage>,
    ) {
        let (tx, rx) = mpsc::channel(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::platform(id);
        let handle = ActorHandle::new(tx, token, metadata);
        (handle, rx)
    }

    fn default_streamer_config(_id: &str) -> StreamerConfig {
        StreamerConfig::default()
    }

    fn default_platform_config(id: &str) -> PlatformConfig {
        PlatformConfig {
            platform_id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_routing_result() {
        let mut result = RoutingResult::new();
        assert_eq!(result.delivered, 0);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());

        result.record_success();
        assert_eq!(result.delivered, 1);
        assert!(result.all_succeeded());

        result.record_failure("test-actor");
        assert_eq!(result.failed, 1);
        assert!(!result.all_succeeded());
        assert_eq!(result.total(), 2);
    }

    #[test]
    fn test_config_scope_from_event() {
        let streamer_event = ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: "streamer-1".to_string(),
        };
        assert_eq!(
            ConfigScope::from_event(&streamer_event),
            ConfigScope::Streamer("streamer-1".to_string())
        );

        let platform_event = ConfigUpdateEvent::PlatformUpdated {
            platform_id: "twitch".to_string(),
        };
        assert_eq!(
            ConfigScope::from_event(&platform_event),
            ConfigScope::Platform("twitch".to_string())
        );

        let global_event = ConfigUpdateEvent::GlobalUpdated;
        assert_eq!(ConfigScope::from_event(&global_event), ConfigScope::Global);
    }

    #[test]
    fn test_platform_mapping() {
        let mut mapping = PlatformMapping::new();
        assert!(mapping.is_empty());

        mapping.register("streamer-1", "twitch");
        mapping.register("streamer-2", "twitch");
        mapping.register("streamer-3", "youtube");

        assert_eq!(mapping.len(), 3);
        assert_eq!(
            mapping.get_platform("streamer-1"),
            Some(&"twitch".to_string())
        );
        assert_eq!(
            mapping.get_platform("streamer-3"),
            Some(&"youtube".to_string())
        );
        assert_eq!(mapping.get_platform("unknown"), None);

        let twitch_streamers = mapping.streamers_on_platform("twitch");
        assert_eq!(twitch_streamers.len(), 2);
        assert!(twitch_streamers.contains(&&"streamer-1".to_string()));
        assert!(twitch_streamers.contains(&&"streamer-2".to_string()));

        mapping.unregister("streamer-1");
        assert_eq!(mapping.len(), 2);
        assert_eq!(mapping.get_platform("streamer-1"), None);
    }

    #[test]
    fn test_get_target_actors_streamer_scope() {
        let (handle1, _rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, _rx2) = create_test_streamer_handle("streamer-2");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);

        let platform_handles = HashMap::new();
        let platform_mapping = PlatformMapping::new();

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        // Existing streamer
        let targets = router.get_target_actors(&ConfigScope::Streamer("streamer-1".to_string()));
        assert_eq!(targets, vec!["streamer-1"]);

        // Non-existing streamer
        let targets = router.get_target_actors(&ConfigScope::Streamer("unknown".to_string()));
        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_target_actors_platform_scope() {
        let (handle1, _rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, _rx2) = create_test_streamer_handle("streamer-2");
        let (handle3, _rx3) = create_test_streamer_handle("streamer-3");
        let (platform_handle, _platform_rx) = create_test_platform_handle("twitch");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);
        streamer_handles.insert("streamer-3".to_string(), handle3);

        let mut platform_handles = HashMap::new();
        platform_handles.insert("twitch".to_string(), platform_handle);

        let mut platform_mapping = PlatformMapping::new();
        platform_mapping.register("streamer-1", "twitch");
        platform_mapping.register("streamer-2", "twitch");
        platform_mapping.register("streamer-3", "youtube");

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let targets = router.get_target_actors(&ConfigScope::Platform("twitch".to_string()));
        assert!(targets.contains(&"platform:twitch".to_string()));
        assert!(targets.contains(&"streamer-1".to_string()));
        assert!(targets.contains(&"streamer-2".to_string()));
        assert!(!targets.contains(&"streamer-3".to_string()));
        assert_eq!(targets.len(), 3); // platform + 2 streamers
    }

    #[test]
    fn test_get_target_actors_global_scope() {
        let (handle1, _rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, _rx2) = create_test_streamer_handle("streamer-2");
        let (platform_handle, _platform_rx) = create_test_platform_handle("twitch");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);

        let mut platform_handles = HashMap::new();
        platform_handles.insert("twitch".to_string(), platform_handle);

        let platform_mapping = PlatformMapping::new();

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let targets = router.get_target_actors(&ConfigScope::Global);
        assert!(targets.contains(&"platform:twitch".to_string()));
        assert!(targets.contains(&"streamer-1".to_string()));
        assert!(targets.contains(&"streamer-2".to_string()));
        assert_eq!(targets.len(), 3);
    }

    #[tokio::test]
    async fn test_route_to_streamer() {
        let (handle1, mut rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, _rx2) = create_test_streamer_handle("streamer-2");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);

        let platform_handles = HashMap::new();
        let platform_mapping = PlatformMapping::new();

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let result = router
            .route_with_scope(
                &ConfigScope::Streamer("streamer-1".to_string()),
                default_streamer_config,
                default_platform_config,
            )
            .await;

        assert_eq!(result.delivered, 1);
        assert_eq!(result.failed, 0);

        // Verify message was received
        let msg = rx1.try_recv().unwrap();
        assert!(matches!(msg, StreamerMessage::ConfigUpdate(_)));
    }

    #[tokio::test]
    async fn test_route_to_platform() {
        let (handle1, mut rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, mut rx2) = create_test_streamer_handle("streamer-2");
        let (handle3, mut rx3) = create_test_streamer_handle("streamer-3");
        let (platform_handle, mut platform_rx) = create_test_platform_handle("twitch");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);
        streamer_handles.insert("streamer-3".to_string(), handle3);

        let mut platform_handles = HashMap::new();
        platform_handles.insert("twitch".to_string(), platform_handle);

        let mut platform_mapping = PlatformMapping::new();
        platform_mapping.register("streamer-1", "twitch");
        platform_mapping.register("streamer-2", "twitch");
        platform_mapping.register("streamer-3", "youtube");

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let result = router
            .route_with_scope(
                &ConfigScope::Platform("twitch".to_string()),
                default_streamer_config,
                default_platform_config,
            )
            .await;

        // Should deliver to platform actor + 2 twitch streamers
        assert_eq!(result.delivered, 3);
        assert_eq!(result.failed, 0);

        // Verify messages were received
        assert!(matches!(
            platform_rx.try_recv().unwrap(),
            PlatformMessage::ConfigUpdate(_)
        ));
        assert!(matches!(
            rx1.try_recv().unwrap(),
            StreamerMessage::ConfigUpdate(_)
        ));
        assert!(matches!(
            rx2.try_recv().unwrap(),
            StreamerMessage::ConfigUpdate(_)
        ));

        // streamer-3 should NOT have received a message (on youtube, not twitch)
        assert!(rx3.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_route_globally() {
        let (handle1, mut rx1) = create_test_streamer_handle("streamer-1");
        let (handle2, mut rx2) = create_test_streamer_handle("streamer-2");
        let (platform_handle, mut platform_rx) = create_test_platform_handle("twitch");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);
        streamer_handles.insert("streamer-2".to_string(), handle2);

        let mut platform_handles = HashMap::new();
        platform_handles.insert("twitch".to_string(), platform_handle);

        let platform_mapping = PlatformMapping::new();

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let result = router
            .route_with_scope(
                &ConfigScope::Global,
                default_streamer_config,
                default_platform_config,
            )
            .await;

        // Should deliver to all actors
        assert_eq!(result.delivered, 3);
        assert_eq!(result.failed, 0);

        // Verify all received messages
        assert!(matches!(
            platform_rx.try_recv().unwrap(),
            PlatformMessage::ConfigUpdate(_)
        ));
        assert!(matches!(
            rx1.try_recv().unwrap(),
            StreamerMessage::ConfigUpdate(_)
        ));
        assert!(matches!(
            rx2.try_recv().unwrap(),
            StreamerMessage::ConfigUpdate(_)
        ));
    }

    #[tokio::test]
    async fn test_route_event() {
        let (handle1, mut rx1) = create_test_streamer_handle("streamer-1");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);

        let platform_handles = HashMap::new();
        let platform_mapping = PlatformMapping::new();

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let event = ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: "streamer-1".to_string(),
        };

        let result = router
            .route_event(&event, default_streamer_config, default_platform_config)
            .await;

        assert_eq!(result.delivered, 1);
        assert!(matches!(
            rx1.try_recv().unwrap(),
            StreamerMessage::ConfigUpdate(_)
        ));
    }

    #[tokio::test]
    async fn test_route_to_stopped_actor() {
        let (handle1, rx1) = create_test_streamer_handle("streamer-1");

        let mut streamer_handles = HashMap::new();
        streamer_handles.insert("streamer-1".to_string(), handle1);

        let platform_handles = HashMap::new();
        let platform_mapping = PlatformMapping::new();

        // Drop the receiver to simulate a stopped actor
        drop(rx1);

        let router = ConfigRouter::new(&streamer_handles, &platform_handles, &platform_mapping);

        let result = router
            .route_with_scope(
                &ConfigScope::Streamer("streamer-1".to_string()),
                default_streamer_config,
                default_platform_config,
            )
            .await;

        // Should record failure
        assert_eq!(result.delivered, 0);
        assert_eq!(result.failed, 1);
        assert!(result.failed_actors.contains(&"streamer-1".to_string()));
    }
}
