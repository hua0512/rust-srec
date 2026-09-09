//! Configuration update events.
//!
//! This module defines events that are broadcast when configurations change,
//! allowing other services to react to configuration updates.

use tokio::sync::broadcast;

/// Events broadcast when configuration changes occur.
///
/// # Event Types and Their Usage
///
/// ## `StreamerMetadataUpdated` vs `StreamerStateSyncedFromDb`
///
/// These two events serve different purposes and are emitted from different code paths:
///
/// - **`StreamerMetadataUpdated`**: Emitted when streamer metadata is changed via API operations
///   (create, update, partial_update). This is for user-initiated changes and may include a
///   **state transition** (e.g., the user disables a streamer). Handlers that manage runtime
///   resources (actors, downloads, danmu, etc.) must treat this as "something about the streamer
///   changed" and consult the latest metadata (e.g., `metadata.is_active()`) to decide whether
///   cleanup is required.
///
/// - **`StreamerStateSyncedFromDb`**: Emitted by committed streamer publication (or explicit reconciliation) after
///   transactional database updates (e.g., monitor detecting errors, session state changes).
///   This is for system-initiated state synchronization. The scheduler uses this to spawn/remove
///   actors without routing config updates.
///
/// They should NOT overlap in normal operation:
/// - API update -> `StreamerMetadataUpdated` only
/// - Transaction sync -> `StreamerStateSyncedFromDb` only
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigUpdateEvent {
    /// Global configuration was updated.
    GlobalUpdated,
    /// A platform configuration was updated.
    PlatformUpdated { platform_id: String },
    /// A template configuration was updated.
    TemplateUpdated { template_id: String },
    /// A streamer was updated via API.
    ///
    /// Emitted by: `create_streamer()`, `update_streamer()`, `partial_update_streamer()`
    ///
    /// This event is intentionally coarse-grained:
    /// - It may represent a config change (name/url/template/priority/etc.)
    /// - It may also represent a user-initiated state change (e.g. DISABLED)
    ///
    /// Handlers should consult the latest `StreamerMetadata` (e.g., `metadata.is_active()`)
    /// to determine whether the streamer became inactive and needs cleanup.
    StreamerMetadataUpdated { streamer_id: String },
    /// A streamer was deleted.
    StreamerDeleted { streamer_id: String },
    /// An engine configuration was updated.
    EngineUpdated { engine_id: String },
    /// A streamer's state was synchronized from the database.
    ///
    /// Emitted by: committed streamer publication after transactional DB updates.
    ///
    /// This event is used by the scheduler to spawn/remove actors based on state changes
    /// that occurred via transactional operations (e.g., monitor error handling).
    /// Unlike `StreamerMetadataUpdated`, this does NOT trigger config routing to actors.
    ///
    /// Note: user-initiated state changes made through the API use `StreamerMetadataUpdated`,
    /// not this event.
    StreamerStateSyncedFromDb {
        streamer_id: String,
        /// Whether the streamer is now active (can be monitored).
        is_active: bool,
    },

    /// Streamer filters were created/updated/deleted.
    ///
    /// Filters are stored separately from the main config/templates and can affect scheduling
    /// decisions (e.g. OutOfSchedule smart-wake). Emit this to force a re-check for the streamer.
    StreamerFiltersUpdated { streamer_id: String },
}

impl ConfigUpdateEvent {
    /// Get a description of the event for logging.
    pub fn description(&self) -> String {
        match self {
            Self::GlobalUpdated => "Global config updated".to_string(),
            Self::PlatformUpdated { platform_id } => {
                format!("Platform config updated: {}", platform_id)
            }
            Self::TemplateUpdated { template_id } => {
                format!("Template config updated: {}", template_id)
            }
            Self::StreamerMetadataUpdated { streamer_id } => {
                format!("Streamer metadata updated: {}", streamer_id)
            }
            Self::StreamerDeleted { streamer_id } => {
                format!("Streamer deleted: {}", streamer_id)
            }
            Self::EngineUpdated { engine_id } => {
                format!("Engine config updated: {}", engine_id)
            }
            Self::StreamerStateSyncedFromDb {
                streamer_id,
                is_active,
            } => {
                format!(
                    "Streamer state synced from DB: {} (active={})",
                    streamer_id, is_active
                )
            }
            Self::StreamerFiltersUpdated { streamer_id } => {
                format!("Streamer filters updated: {}", streamer_id)
            }
        }
    }
}

/// Default channel capacity for config update events.
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Broadcaster for configuration update events.
///
/// Uses tokio's broadcast channel to distribute events to multiple subscribers.
pub struct ConfigEventBroadcaster {
    sender: broadcast::Sender<ConfigUpdateEvent>,
}

impl ConfigEventBroadcaster {
    /// Create a new broadcaster with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    /// Create a new broadcaster with specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to configuration update events.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigUpdateEvent> {
        self.sender.subscribe()
    }

    /// Publish a configuration update event.
    ///
    /// Returns the number of receivers that received the event.
    /// Returns 0 if there are no active subscribers.
    pub fn publish(&self, event: ConfigUpdateEvent) -> usize {
        tracing::debug!("Publishing config event: {}", event.description());
        // send() returns Err if there are no receivers, which is fine
        self.sender.send(event).unwrap_or(0)
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for ConfigEventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ConfigEventBroadcaster {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_description() {
        assert_eq!(
            ConfigUpdateEvent::GlobalUpdated.description(),
            "Global config updated"
        );
        assert_eq!(
            ConfigUpdateEvent::PlatformUpdated {
                platform_id: "twitch".to_string()
            }
            .description(),
            "Platform config updated: twitch"
        );
    }

    #[tokio::test]
    async fn test_broadcaster_publish_subscribe() {
        let broadcaster = ConfigEventBroadcaster::new();
        let mut receiver = broadcaster.subscribe();

        let event = ConfigUpdateEvent::GlobalUpdated;
        let count = broadcaster.publish(event.clone());
        assert_eq!(count, 1);

        let received = receiver.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_broadcaster_multiple_subscribers() {
        let broadcaster = ConfigEventBroadcaster::new();
        let mut receiver1 = broadcaster.subscribe();
        let mut receiver2 = broadcaster.subscribe();

        assert_eq!(broadcaster.subscriber_count(), 2);

        let event = ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: "streamer-1".to_string(),
        };
        let count = broadcaster.publish(event.clone());
        assert_eq!(count, 2);

        assert_eq!(receiver1.recv().await.unwrap(), event);
        assert_eq!(receiver2.recv().await.unwrap(), event);
    }

    #[test]
    fn test_broadcaster_no_subscribers() {
        let broadcaster = ConfigEventBroadcaster::new();
        // Publishing with no subscribers should not panic
        let count = broadcaster.publish(ConfigUpdateEvent::GlobalUpdated);
        assert_eq!(count, 0);
    }
}
