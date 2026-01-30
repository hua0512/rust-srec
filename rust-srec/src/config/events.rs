//! Configuration update events.
//!
//! This module defines events that are broadcast when configurations change,
//! allowing other services to react to configuration updates.
//!
//! Includes update coalescing to batch rapid updates within a time window.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, broadcast};

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
/// - **`StreamerStateSyncedFromDb`**: Emitted by `StreamerManager::reload_from_repo()` after
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
    /// Emitted by: `StreamerManager::reload_from_repo()` after transactional DB updates.
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

/// Default coalescing window (100ms).
const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_millis(100);

/// Update coalescer that batches rapid configuration updates.
///
/// When multiple updates occur within a short time window, they are
/// coalesced into a single batch to reduce unnecessary cache invalidations.
pub struct UpdateCoalescer {
    /// Pending events waiting to be flushed.
    pending: Arc<Mutex<PendingUpdates>>,
    /// Coalescing window duration.
    window: Duration,
    /// Broadcaster to send coalesced events.
    broadcaster: ConfigEventBroadcaster,
}

/// Pending updates waiting to be coalesced.
struct PendingUpdates {
    /// Set of pending events (deduplicated).
    events: HashSet<ConfigUpdateEvent>,
    /// When the first event was added.
    first_event_time: Option<Instant>,
    /// Whether a global update is pending (supersedes all others).
    has_global: bool,
}

impl PendingUpdates {
    fn new() -> Self {
        Self {
            events: HashSet::new(),
            first_event_time: None,
            has_global: false,
        }
    }

    fn add(&mut self, event: ConfigUpdateEvent) {
        if self.first_event_time.is_none() {
            self.first_event_time = Some(Instant::now());
        }

        if matches!(event, ConfigUpdateEvent::GlobalUpdated) {
            self.has_global = true;
        }

        self.events.insert(event);
    }

    fn should_flush(&self, window: Duration) -> bool {
        self.first_event_time
            .map(|t| t.elapsed() >= window)
            .unwrap_or(false)
    }

    fn take(&mut self) -> Vec<ConfigUpdateEvent> {
        let events: Vec<_> = if self.has_global {
            // Global update supersedes all others
            vec![ConfigUpdateEvent::GlobalUpdated]
        } else {
            self.events.drain().collect()
        };

        self.first_event_time = None;
        self.has_global = false;

        events
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl std::hash::Hash for ConfigUpdateEvent {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            ConfigUpdateEvent::GlobalUpdated => {}
            ConfigUpdateEvent::PlatformUpdated { platform_id } => platform_id.hash(state),
            ConfigUpdateEvent::TemplateUpdated { template_id } => template_id.hash(state),
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => streamer_id.hash(state),
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => streamer_id.hash(state),
            ConfigUpdateEvent::EngineUpdated { engine_id } => engine_id.hash(state),
            ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id,
                is_active,
            } => {
                streamer_id.hash(state);
                is_active.hash(state);
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => streamer_id.hash(state),
        }
    }
}

impl UpdateCoalescer {
    /// Create a new update coalescer with default window.
    pub fn new(broadcaster: ConfigEventBroadcaster) -> Self {
        Self::with_window(broadcaster, DEFAULT_COALESCE_WINDOW)
    }

    /// Create a new update coalescer with specified window.
    pub fn with_window(broadcaster: ConfigEventBroadcaster, window: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(PendingUpdates::new())),
            window,
            broadcaster,
        }
    }

    /// Queue an event for coalescing.
    ///
    /// The event will be held until the coalescing window expires,
    /// then flushed along with any other pending events.
    pub async fn queue(&self, event: ConfigUpdateEvent) {
        let mut pending = self.pending.lock().await;
        pending.add(event);

        // Check if we should flush
        if pending.should_flush(self.window) {
            let events = pending.take();
            drop(pending); // Release lock before broadcasting

            for event in events {
                self.broadcaster.publish(event);
            }
        }
    }

    /// Force flush all pending events immediately.
    pub async fn flush(&self) {
        let mut pending = self.pending.lock().await;
        if pending.is_empty() {
            return;
        }

        let events = pending.take();
        drop(pending);

        for event in events {
            self.broadcaster.publish(event);
        }
    }

    /// Get the number of pending events.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.events.len()
    }

    /// Start a background task that periodically flushes pending events.
    pub fn start_flush_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let coalescer = self;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(coalescer.window);
            loop {
                interval.tick().await;
                coalescer.flush().await;
            }
        })
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

    #[tokio::test]
    async fn test_coalescer_deduplicates_events() {
        let broadcaster = ConfigEventBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        let coalescer = UpdateCoalescer::with_window(broadcaster, Duration::from_millis(50));

        // Queue duplicate events
        let event = ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: "streamer-1".to_string(),
        };
        coalescer.queue(event.clone()).await;
        coalescer.queue(event.clone()).await;
        coalescer.queue(event.clone()).await;

        // Should have only 1 pending (deduplicated)
        assert_eq!(coalescer.pending_count().await, 1);

        // Flush and verify only one event is sent
        coalescer.flush().await;

        let received = receiver.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_coalescer_global_supersedes_all() {
        let broadcaster = ConfigEventBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        let coalescer = UpdateCoalescer::with_window(broadcaster, Duration::from_millis(50));

        // Queue various events
        coalescer
            .queue(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: "streamer-1".to_string(),
            })
            .await;
        coalescer
            .queue(ConfigUpdateEvent::PlatformUpdated {
                platform_id: "twitch".to_string(),
            })
            .await;
        coalescer.queue(ConfigUpdateEvent::GlobalUpdated).await;

        // Flush - should only get GlobalUpdated
        coalescer.flush().await;

        let received = receiver.recv().await.unwrap();
        assert_eq!(received, ConfigUpdateEvent::GlobalUpdated);

        // No more events
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_coalescer_auto_flush_on_window() {
        let broadcaster = ConfigEventBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        let coalescer = UpdateCoalescer::with_window(broadcaster, Duration::from_millis(20));

        // Queue an event
        coalescer.queue(ConfigUpdateEvent::GlobalUpdated).await;

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Queue another event - should trigger flush of first
        coalescer
            .queue(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: "test".to_string(),
            })
            .await;

        // Should have received the global update
        let received = receiver.recv().await.unwrap();
        assert_eq!(received, ConfigUpdateEvent::GlobalUpdated);
    }
}
