//! Notification service implementation.
//!
//! The NotificationService is responsible for:
//! - Listening to system events (Monitor, Download, Pipeline)
//! - Dispatching notifications to configured channels
//! - Managing retry logic with exponential backoff
//! - Implementing circuit breaker pattern for failing channels
//! - Maintaining a dead letter queue for failed notifications

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::channels::ChannelConfig;
#[cfg(test)]
use super::channels::NotificationChannel;
use super::events::NotificationEvent;
use super::web_push::WebPushService;
use crate::Result;
#[cfg(test)]
use crate::database::models::NotificationChannelDbModel;
use crate::database::models::NotificationEventLogDbModel;
use crate::database::repositories::NotificationRepository;
use crate::downloader::DownloadManagerEvent;
#[cfg(test)]
use crate::downloader::DownloadProgressEvent;
use crate::monitor::MonitorEvent;
use crate::pipeline::PipelineEvent;
use crate::utils::task_supervisor::TaskSupervisor;

mod dead_letter;
mod delivery;
mod listeners;
mod registry;

#[cfg(test)]
use registry::parse_channel_locale;
use registry::{ChannelRegistry, RuntimeChannel};
mod web_push_queue;

/// Best-effort interval for in-memory dead-letter cleanup.
///
/// Dead letters are also persisted to the database; this is purely to prevent unbounded growth
/// of the in-memory `dead_letters` map in long-running processes.
const DEAD_LETTER_CLEANUP_INTERVAL_SECS: u64 = 60 * 60;
const WEB_PUSH_QUEUE_CAPACITY: usize = 2048;
const WEB_PUSH_BATCH_SIZE: usize = 64;
const WEB_PUSH_FLUSH_INTERVAL_MS: u64 = 250;

/// Configuration for the notification service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationServiceConfig {
    /// Whether the notification service is enabled.
    pub enabled: bool,
    /// Maximum pending notifications; zero disables ordinary channel admission.
    pub max_queue_size: usize,
    /// Maximum retry attempts per notification.
    pub max_retries: u32,
    /// Initial retry delay in milliseconds.
    pub initial_retry_delay_ms: u64,
    /// Maximum retry delay in milliseconds.
    pub max_retry_delay_ms: u64,
    /// Circuit breaker failure threshold.
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker cooldown in seconds.
    pub circuit_breaker_cooldown_secs: u64,
    /// Dead letter retention in days.
    pub dead_letter_retention_days: u32,
    /// Channel configurations.
    pub channels: Vec<ChannelConfig>,
}

impl Default for NotificationServiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_queue_size: 1000,
            max_retries: 3,
            initial_retry_delay_ms: 5000,
            max_retry_delay_ms: 60000,
            circuit_breaker_threshold: 10,
            circuit_breaker_cooldown_secs: 300,
            dead_letter_retention_days: 7,
            channels: Vec::new(),
        }
    }
}

/// Circuit breaker state for a channel.
#[derive(Debug, Clone)]
struct CircuitBreakerState {
    /// Number of consecutive failures.
    failures: u32,
    /// Whether the circuit is open (disabled).
    is_open: bool,
    /// When the circuit was opened.
    opened_at: Option<DateTime<Utc>>,
    /// Cooldown duration.
    cooldown: Duration,
}

impl CircuitBreakerState {
    fn new(cooldown_secs: u64) -> Self {
        Self {
            failures: 0,
            is_open: false,
            opened_at: None,
            cooldown: Duration::from_secs(cooldown_secs),
        }
    }

    fn record_failure(&mut self, threshold: u32) {
        self.failures += 1;

        // If already open (or in the "cooldown passed but not yet recovered" state),
        // restart the cooldown on any failure so we don't allow unlimited attempts.
        if self.is_open {
            self.opened_at = Some(Utc::now());
            return;
        }

        if self.failures >= threshold && !self.is_open {
            self.is_open = true;
            self.opened_at = Some(Utc::now());
            warn!("Circuit breaker opened after {} failures", self.failures);
        }
    }

    fn record_success(&mut self) {
        self.failures = 0;
        self.is_open = false;
        self.opened_at = None;
    }

    fn is_allowed(&self) -> bool {
        if !self.is_open {
            return true;
        }

        // Check if cooldown has passed (half-open state)
        if let Some(opened_at) = self.opened_at {
            let elapsed = Utc::now().signed_duration_since(opened_at);
            if elapsed.num_seconds().max(0) as u64 >= self.cooldown.as_secs() {
                return true; // Allow one request to test recovery
            }
        }

        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryStatus {
    Pending,
    Delivered,
    DeadLettered,
}

#[derive(Debug, Clone)]
struct ChannelDeliveryState {
    status: DeliveryStatus,
    attempts: u32,
    last_attempt: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

/// A notification pending delivery.
#[derive(Clone)]
struct PendingNotification {
    _id: u64,
    event: NotificationEvent,
    created_at: DateTime<Utc>,
    channel_state: HashMap<String, ChannelDeliveryState>,
    /// Delivery and retries keep the instance selected at admission, across reloads.
    targets: Vec<Arc<RuntimeChannel>>,
    retry_generation: u64,
    retry_cancel: CancellationToken,
    next_retry_at: Option<DateTime<Utc>>,
}

/// Dead letter entry for failed notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// Dead letter entry ID.
    pub id: u64,
    /// Notification ID.
    pub notification_id: u64,
    /// The event that failed.
    pub event: NotificationEvent,
    /// Channel instance key (config/dynamic/db).
    ///
    /// This is the first-class identifier used internally for delivery tracking, retries,
    /// and circuit breakers.
    pub channel_key: Option<String>,
    /// Channel ID (DB-backed channels only).
    pub channel_id: Option<String>,
    /// Channel that failed.
    pub channel_type: String,
    /// Number of attempts made.
    pub attempts: u32,
    /// Last error message.
    pub error: String,
    /// When the notification was created.
    pub created_at: DateTime<Utc>,
    /// When it was moved to dead letter.
    pub dead_lettered_at: DateTime<Utc>,
}

/// The notification service.
pub struct NotificationService {
    /// Shared with every detached delivery/retry task via
    /// `delivery::DeliveryContext`; `Arc` so those clones don't deep-copy
    /// `NotificationServiceConfig::channels` per attempt.
    config: Arc<NotificationServiceConfig>,
    notification_repo: Option<Arc<dyn NotificationRepository>>,
    web_push_service: Option<Arc<WebPushService>>,
    web_push_tx: parking_lot::RwLock<Option<mpsc::Sender<WebPushQueuedEvent>>>,
    web_push_worker_started: AtomicBool,
    web_push_dropped: AtomicU64,
    registry: RwLock<ChannelRegistry>,
    reload_gate: tokio::sync::Mutex<()>,
    pending_queue: Arc<DashMap<u64, PendingNotification>>,
    queue_admission: parking_lot::Mutex<()>,
    dead_letters: Arc<DashMap<u64, DeadLetterEntry>>,
    /// Last time we performed in-memory dead-letter retention cleanup (unix epoch seconds).
    dead_letter_cleanup_ts: Arc<AtomicU64>,
    next_id: AtomicU64,
    next_dead_letter_id: Arc<AtomicU64>,
    event_tx: broadcast::Sender<NotificationEvent>,
    cancellation_token: CancellationToken,
    task_supervisor: Arc<TaskSupervisor>,
    owns_task_supervisor: bool,
}

#[derive(Debug, Clone)]
struct WebPushQueuedEvent {
    event: NotificationEvent,
    event_log_id: Option<String>,
}

/// Public view of a configured notification channel instance.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelInstance {
    /// Unique channel instance key.
    pub key: String,
    /// DB channel id, when backed by `notification_channel`.
    pub channel_id: Option<String>,
    /// Human-friendly display name.
    pub display_name: String,
    /// Channel type (Discord/Email/Webhook).
    pub channel_type: String,
    /// Channel configuration source.
    pub source: NotificationChannelSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannelSource {
    Config,
    Dynamic,
    Database,
}

impl NotificationService {
    /// Create a new notification service.
    pub fn new() -> Self {
        Self::with_config(NotificationServiceConfig::default())
    }

    pub fn with_repository(
        config: NotificationServiceConfig,
        notification_repo: Arc<dyn NotificationRepository>,
    ) -> Self {
        let mut service = Self::with_config(config);
        service.notification_repo = Some(notification_repo);
        service
    }

    pub fn with_web_push_service(mut self, service: Arc<WebPushService>) -> Self {
        self.web_push_service = Some(service);
        self
    }

    pub(crate) fn with_task_supervisor(mut self, task_supervisor: Arc<TaskSupervisor>) -> Self {
        self.task_supervisor = task_supervisor;
        self.owns_task_supervisor = false;
        self
    }

    /// Create a new notification service with custom configuration.
    pub fn with_config(config: NotificationServiceConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        let service = Self {
            notification_repo: None,
            web_push_service: None,
            web_push_tx: parking_lot::RwLock::new(None),
            web_push_worker_started: AtomicBool::new(false),
            web_push_dropped: AtomicU64::new(0),
            registry: RwLock::new(ChannelRegistry::default()),
            reload_gate: tokio::sync::Mutex::new(()),
            pending_queue: Arc::new(DashMap::new()),
            queue_admission: parking_lot::Mutex::new(()),
            dead_letters: Arc::new(DashMap::new()),
            dead_letter_cleanup_ts: Arc::new(AtomicU64::new(0)),
            next_id: AtomicU64::new(1),
            next_dead_letter_id: Arc::new(AtomicU64::new(1)),
            event_tx,
            cancellation_token: CancellationToken::new(),
            task_supervisor: Arc::new(TaskSupervisor::new()),
            owns_task_supervisor: true,
            config: Arc::new(config),
        };

        // Initialize channels from config
        service.init_channels();

        service
    }
    /// Subscribe to notification events.
    pub fn subscribe(&self) -> broadcast::Receiver<NotificationEvent> {
        self.event_tx.subscribe()
    }

    /// Send a notification to a specific channel instance (bypasses DB subscriptions).
    pub async fn notify_channel_instance(&self, key: &str, event: NotificationEvent) -> Result<()> {
        self.notify_channel_instances(std::iter::once(key.to_string()).collect(), event)
            .await
    }

    async fn notify_channel_instances(
        &self,
        keys: HashSet<String>,
        event: NotificationEvent,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let _ = self.event_tx.send(event.clone());

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let target_channels = {
            let registry = self.registry.read();
            keys.iter()
                .map(|key| {
                    registry
                        .by_key
                        .get(key)
                        .cloned()
                        .ok_or_else(|| crate::Error::NotFound {
                            entity_type: "NotificationChannelInstance".to_owned(),
                            id: key.clone(),
                        })
                })
                .collect::<Result<Vec<_>>>()?
        };

        if target_channels.is_empty() {
            return Ok(());
        }

        let mut channel_state = HashMap::new();
        for channel in &target_channels {
            channel_state
                .entry(channel.key.clone())
                .or_insert(ChannelDeliveryState {
                    status: DeliveryStatus::Pending,
                    attempts: 0,
                    last_attempt: None,
                    last_error: None,
                });
        }

        let pending = PendingNotification {
            _id: id,
            event,
            created_at: Utc::now(),
            channel_state,
            targets: target_channels,
            retry_generation: 0,
            retry_cancel: CancellationToken::new(),
            next_retry_at: None,
        };

        if self.enqueue_pending(id, pending) {
            self.process_notification(id).await;
        }
        Ok(())
    }

    fn enqueue_pending(&self, id: u64, pending: PendingNotification) -> bool {
        // Capacity inspection, oldest selection and insertion form one admission.
        // Delivery may remove entries concurrently, but cannot add capacity.
        let _admission = self.queue_admission.lock();
        if self.config.max_queue_size == 0 {
            return false;
        }
        if self.pending_queue.len() >= self.config.max_queue_size {
            let oldest_id = self
                .pending_queue
                .iter()
                .min_by_key(|entry| (entry.created_at, *entry.key()))
                .map(|entry| *entry.key());
            if let Some(oldest_id) = oldest_id
                && let Some((_, evicted)) = self.pending_queue.remove(&oldest_id)
            {
                evicted.retry_cancel.cancel();
                warn!(
                    notification_id = oldest_id,
                    "Notification queue full, dropping oldest notification"
                );
            }
        }
        self.pending_queue.insert(id, pending);
        true
    }

    /// Send a notification to all enabled channels.
    pub async fn notify(&self, event: NotificationEvent) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Broadcast the event internally
        let _ = self.event_tx.send(event.clone());

        // Best-effort: persist event log for UI/debugging/audit.
        let event_log_id = Uuid::new_v4().to_string();
        if let Some(repo) = self.notification_repo.as_ref().cloned() {
            let entry = NotificationEventLogDbModel {
                id: event_log_id.clone(),
                event_type: event.event_type().to_string(),
                priority: event.priority().as_int() as i32,
                payload: serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string()),
                streamer_id: event.streamer_id().map(|s| s.to_string()),
                created_at: event.timestamp().timestamp_millis(),
            };
            if let Err(e) = repo.add_event_log(&entry).await {
                warn!(error = %e, "Failed to persist notification event log (non-fatal)");
            }
        }

        // Best-effort: send web push notifications (independent of channel delivery).
        if self.web_push_service.is_some() {
            let queued = WebPushQueuedEvent {
                event: event.clone(),
                event_log_id: Some(event_log_id.clone()),
            };

            self.enqueue_web_push(queued);
        }

        // Queue the notification
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let target_channels: Vec<Arc<RuntimeChannel>> = {
            let registry = self.registry.read();
            let subscribed = registry.subscriptions_by_event.get(event.event_type());
            registry
                .channels
                .iter()
                .filter(|channel| match &channel.db_channel_id {
                    None => true,
                    Some(id) => subscribed.is_some_and(|ids| ids.contains(id)),
                })
                .cloned()
                .collect()
        };

        if target_channels.is_empty() {
            return Ok(());
        }

        let mut channel_state = HashMap::new();
        for channel in &target_channels {
            channel_state
                .entry(channel.key.clone())
                .or_insert(ChannelDeliveryState {
                    status: DeliveryStatus::Pending,
                    attempts: 0,
                    last_attempt: None,
                    last_error: None,
                });
        }

        let pending = PendingNotification {
            _id: id,
            event: event.clone(),
            created_at: Utc::now(),
            channel_state,
            targets: target_channels,
            retry_generation: 0,
            retry_cancel: CancellationToken::new(),
            next_retry_at: None,
        };

        if self.enqueue_pending(id, pending) {
            self.process_notification(id).await;
        }
        Ok(())
    }

    /// Get queue statistics.
    pub fn stats(&self) -> NotificationStats {
        let registry = self.registry.read();
        NotificationStats {
            pending_count: self.pending_queue.len(),
            dead_letter_count: self.dead_letters.len(),
            channel_count: registry.channels.len(),
            web_push_dropped: self.web_push_dropped.load(Ordering::Relaxed),
            circuit_breakers: registry
                .channels
                .iter()
                .map(|channel| (channel.key.clone(), channel.breaker.lock().is_open))
                .collect(),
        }
    }

    /// Start listening for system events.
    pub fn start_event_listeners(
        self: &Arc<Self>,
        monitor_rx: broadcast::Receiver<MonitorEvent>,
        download_rx: broadcast::Receiver<DownloadManagerEvent>,
        pipeline_rx: broadcast::Receiver<PipelineEvent>,
        session_rx: broadcast::Receiver<crate::session::SessionTransition>,
    ) {
        self.listen_for_monitor_events(monitor_rx);
        self.listen_for_download_events(download_rx);
        self.listen_for_pipeline_events(pipeline_rx);
        self.listen_for_session_transitions(session_rx);
    }

    pub(crate) fn dispatch_notification(self: &Arc<Self>, notification: NotificationEvent) {
        let service = self.clone();
        self.task_supervisor
            .spawn("notification dispatch", async move {
                if let Err(error) = service.notify(notification).await {
                    warn!(error = %error, "Failed to dispatch notification");
                }
            });
    }

    /// Stop the notification service.
    pub async fn stop(&self) {
        info!("Stopping notification service");
        // Fence admission before waking the worker to drain. A sender that already
        // holds the read lock must publish before cancellation becomes visible.
        {
            let mut sender = self.web_push_tx.write();
            self.cancellation_token.cancel();
            sender.take();
        }
        if self.owns_task_supervisor
            && !self.task_supervisor.shutdown(Duration::from_secs(10)).await
        {
            warn!("Notification background task shutdown required forced cancellation");
        }

        // Process any remaining notifications
        let pending_ids: Vec<u64> = self.pending_queue.iter().map(|e| *e.key()).collect();
        for id in pending_ids {
            self.process_notification(id).await;
        }

        info!("Notification service stopped");
    }
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the notification service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationStats {
    /// Web-push events rejected by queue admission (all priorities). No automatic replay.
    #[serde(default)]
    pub web_push_dropped: u64,
    /// Number of pending notifications.
    pub pending_count: usize,
    /// Number of dead letter entries.
    pub dead_letter_count: usize,
    /// Number of configured channels.
    pub channel_count: usize,
    /// Circuit breaker states (channel_key -> is_open).
    pub circuit_breakers: HashMap<String, bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::NotificationDeadLetterDbModel;
    use crate::notification::channels::DiscordConfig;

    mod delivery_contracts;
    mod listeners;
    mod reload;
    mod rendering;

    #[test]
    fn parse_channel_locale_reads_the_settings_blob() {
        assert_eq!(
            parse_channel_locale(&serde_json::json!({ "locale": "zh-CN" })),
            Some("zh-CN".to_string())
        );
        assert_eq!(
            parse_channel_locale(&serde_json::json!({ "locale": "  zh-CN  " })),
            Some("zh-CN".to_string())
        );
    }

    #[test]
    fn parse_channel_locale_treats_blank_as_unset() {
        // A select cleared back to "follow the server" posts an empty string rather than
        // dropping the key, and that must not reach `t!` as a locale.
        for settings in [
            serde_json::json!({ "locale": "" }),
            serde_json::json!({ "locale": "   " }),
            serde_json::json!({ "locale": null }),
            serde_json::json!({ "min_priority": 5 }),
        ] {
            assert_eq!(parse_channel_locale(&settings), None, "{settings}");
        }
    }

    #[test]
    fn test_notification_service_creation() {
        let service = NotificationService::new();
        let stats = service.stats();
        assert_eq!(stats.pending_count, 0);
        assert_eq!(stats.dead_letter_count, 0);
    }

    #[test]
    fn test_circuit_breaker_state() {
        let mut cb = CircuitBreakerState::new(300);
        assert!(cb.is_allowed());

        // Record failures up to threshold
        for _ in 0..10 {
            cb.record_failure(10);
        }
        assert!(!cb.is_allowed());

        // Record success resets
        cb.record_success();
        assert!(cb.is_allowed());
    }

    #[test]
    fn test_calculate_retry_delay() {
        let config = NotificationServiceConfig {
            initial_retry_delay_ms: 1000,
            max_retry_delay_ms: 60000,
            ..Default::default()
        };
        let service = NotificationService::with_config(config);

        let delay1 = service._calculate_retry_delay(0);
        let delay2 = service._calculate_retry_delay(1);
        let delay3 = service._calculate_retry_delay(2);

        // Delays should increase (approximately, due to jitter)
        assert!(delay1.as_millis() >= 750 && delay1.as_millis() <= 1250);
        assert!(delay2.as_millis() >= 1500 && delay2.as_millis() <= 2500);
        assert!(delay3.as_millis() >= 3000 && delay3.as_millis() <= 5000);
    }

    #[tokio::test]
    async fn test_notify_disabled() {
        let config = NotificationServiceConfig {
            enabled: false,
            ..Default::default()
        };
        let service = NotificationService::with_config(config);

        let event = NotificationEvent::SystemStartup {
            version: "test".to_string(),
            timestamp: Utc::now(),
        };

        // Should succeed but not queue anything
        service.notify(event).await.unwrap();
        assert_eq!(service.stats().pending_count, 0);
    }

    #[test]
    fn test_add_channel() {
        let service = NotificationService::new();

        let config = ChannelConfig::Discord(DiscordConfig {
            enabled: true,
            webhook_url: "https://discord.com/api/webhooks/test".to_string(),
            ..Default::default()
        });

        service.add_channel(config);
        assert_eq!(service.stats().channel_count, 1);
    }

    struct TestChannel {
        channel_type: &'static str,
        fail_for_attempts: u32,
        attempts: Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait::async_trait]
    impl NotificationChannel for TestChannel {
        fn channel_type(&self) -> &'static str {
            self.channel_type
        }

        fn is_enabled(&self) -> bool {
            true
        }

        async fn send(&self, _event: &NotificationEvent) -> Result<()> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.fail_for_attempts {
                Err(crate::Error::Other(format!("forced failure {}", attempt)))
            } else {
                Ok(())
            }
        }

        async fn test(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn retries_do_not_duplicate_successful_channels() {
        let config = NotificationServiceConfig {
            enabled: true,
            max_retries: 3,
            initial_retry_delay_ms: 5,
            max_retry_delay_ms: 20,
            circuit_breaker_threshold: 100,
            circuit_breaker_cooldown_secs: 1,
            ..Default::default()
        };
        let service = NotificationService::with_config(config);

        let ok_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let flaky_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let ok_channel = Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: "ok".to_string(),
            db_channel_id: None,
            display_name: "ok".to_string(),
            channel_type: "test".to_string(),
            channel: Arc::new(TestChannel {
                channel_type: "ok",
                fail_for_attempts: 0,
                attempts: ok_attempts.clone(),
            }),
        });
        service.registry.write().insert(ok_channel);

        let flaky_channel = Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: "flaky".to_string(),
            db_channel_id: None,
            display_name: "flaky".to_string(),
            channel_type: "test".to_string(),
            channel: Arc::new(TestChannel {
                channel_type: "flaky",
                fail_for_attempts: 1,
                attempts: flaky_attempts.clone(),
            }),
        });
        service.registry.write().insert(flaky_channel);

        let event = NotificationEvent::SystemStartup {
            version: "test".to_string(),
            timestamp: Utc::now(),
        };

        service.notify(event).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(ok_attempts.load(Ordering::SeqCst), 1);
        assert!(flaky_attempts.load(Ordering::SeqCst) >= 2);
        assert_eq!(service.stats().pending_count, 0);
    }

    #[test]
    fn test_segment_notifications_preserve_lifecycle_timestamps() {
        use crate::notification::events::mapping::download_notification;
        let started_at = DateTime::from_timestamp_millis(1_788_784_496_123).unwrap();
        let completed_at = started_at + chrono::Duration::seconds(10);
        let events = [
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                download_id: "dl-1".into(),
                streamer_id: "streamer-1".into(),
                streamer_name: "Streamer".into(),
                session_id: "session-1".into(),
                segment_path: "/tmp/seg.ts".into(),
                segment_index: 1,
                started_at,
            }),
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id: "dl-1".into(),
                streamer_id: "streamer-1".into(),
                streamer_name: "Streamer".into(),
                session_id: "session-1".into(),
                segment_path: "/tmp/seg.ts".into(),
                segment_index: 1,
                started_at: Some(started_at),
                completed_at,
                duration_secs: 10.0,
                size_bytes: 1024,
                split_reason_code: None,
                split_reason_details_json: None,
            }),
        ];
        for (event, expected_time) in events.into_iter().zip([started_at, completed_at]) {
            let notification =
                download_notification(event, completed_at + chrono::Duration::hours(1)).unwrap();
            assert_eq!(notification.timestamp(), expected_time);
            assert_eq!(notification.streamer_id(), Some("streamer-1"));
        }
    }

    struct MockNotificationRepo {
        channels: tokio::sync::Mutex<Vec<NotificationChannelDbModel>>,
        subscriptions: tokio::sync::Mutex<HashMap<String, Vec<String>>>,
        subscribe_calls: tokio::sync::Mutex<Vec<(String, String)>>,
        unsubscribe_calls: tokio::sync::Mutex<Vec<(String, String)>>,
        dead_letters: tokio::sync::Mutex<Vec<NotificationDeadLetterDbModel>>,
        fail_dead_letter_insert: bool,
        pause_next_list: AtomicBool,
        list_entered: tokio::sync::Notify,
        list_release: tokio::sync::Notify,
        list_calls: std::sync::atomic::AtomicUsize,
        fail_subscriptions_for: parking_lot::Mutex<Option<String>>,
        fail_subscribe: AtomicBool,
    }

    impl MockNotificationRepo {
        fn new() -> Self {
            Self {
                channels: tokio::sync::Mutex::new(Vec::new()),
                subscriptions: tokio::sync::Mutex::new(HashMap::new()),
                subscribe_calls: tokio::sync::Mutex::new(Vec::new()),
                unsubscribe_calls: tokio::sync::Mutex::new(Vec::new()),
                dead_letters: tokio::sync::Mutex::new(Vec::new()),
                fail_dead_letter_insert: false,
                pause_next_list: AtomicBool::new(false),
                list_entered: tokio::sync::Notify::new(),
                list_release: tokio::sync::Notify::new(),
                list_calls: std::sync::atomic::AtomicUsize::new(0),
                fail_subscriptions_for: parking_lot::Mutex::new(None),
                fail_subscribe: AtomicBool::new(false),
            }
        }
    }

    #[async_trait::async_trait]
    impl NotificationRepository for MockNotificationRepo {
        async fn get_channel(&self, _id: &str) -> Result<NotificationChannelDbModel> {
            unimplemented!()
        }

        async fn list_channels(&self) -> Result<Vec<NotificationChannelDbModel>> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            let channels = self.channels.lock().await.clone();
            if self.pause_next_list.swap(false, Ordering::SeqCst) {
                self.list_entered.notify_one();
                self.list_release.notified().await;
            }
            Ok(channels)
        }

        async fn create_channel(&self, _channel: &NotificationChannelDbModel) -> Result<()> {
            unimplemented!()
        }

        async fn update_channel(&self, _channel: &NotificationChannelDbModel) -> Result<()> {
            unimplemented!()
        }

        async fn delete_channel(&self, _id: &str) -> Result<()> {
            unimplemented!()
        }

        async fn get_subscriptions_for_channel(&self, _channel_id: &str) -> Result<Vec<String>> {
            if self.fail_subscriptions_for.lock().as_deref() == Some(_channel_id) {
                return Err(crate::Error::Other(
                    "injected subscription read failure".to_owned(),
                ));
            }
            Ok(self
                .subscriptions
                .lock()
                .await
                .get(_channel_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn get_channels_for_event(
            &self,
            _event_name: &str,
        ) -> Result<Vec<NotificationChannelDbModel>> {
            unimplemented!()
        }

        async fn subscribe(&self, _channel_id: &str, _event_name: &str) -> Result<()> {
            if self.fail_subscribe.load(Ordering::SeqCst) {
                return Err(crate::Error::Other(
                    "injected subscription write failure".to_owned(),
                ));
            }
            self.subscribe_calls
                .lock()
                .await
                .push((_channel_id.to_string(), _event_name.to_string()));

            let mut subs = self.subscriptions.lock().await;
            let entry = subs.entry(_channel_id.to_string()).or_default();
            if !entry.iter().any(|e| e == _event_name) {
                entry.push(_event_name.to_string());
            }
            Ok(())
        }

        async fn unsubscribe(&self, _channel_id: &str, _event_name: &str) -> Result<()> {
            self.unsubscribe_calls
                .lock()
                .await
                .push((_channel_id.to_string(), _event_name.to_string()));

            let mut subs = self.subscriptions.lock().await;
            if let Some(list) = subs.get_mut(_channel_id) {
                list.retain(|e| e != _event_name);
            }
            Ok(())
        }

        async fn unsubscribe_all(&self, _channel_id: &str) -> Result<()> {
            unimplemented!()
        }

        async fn add_to_dead_letter(&self, entry: &NotificationDeadLetterDbModel) -> Result<()> {
            if self.fail_dead_letter_insert {
                return Err(crate::Error::Other(
                    "forced dead letter insert failure".into(),
                ));
            }
            self.dead_letters.lock().await.push(entry.clone());
            Ok(())
        }

        async fn list_dead_letters(
            &self,
            _channel_id: Option<&str>,
            _limit: i32,
        ) -> Result<Vec<NotificationDeadLetterDbModel>> {
            unimplemented!()
        }

        async fn get_dead_letter(&self, _id: &str) -> Result<NotificationDeadLetterDbModel> {
            unimplemented!()
        }

        async fn delete_dead_letter(&self, _id: &str) -> Result<()> {
            unimplemented!()
        }

        async fn add_event_log(&self, _entry: &NotificationEventLogDbModel) -> Result<()> {
            Ok(())
        }

        async fn list_event_logs(
            &self,
            _event_type: Option<&str>,
            _streamer_id: Option<&str>,
            _search: Option<&str>,
            _priority: Option<&str>,
            _offset: i32,
            _limit: i32,
        ) -> Result<Vec<NotificationEventLogDbModel>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn db_subscriptions_filter_delivery() {
        let repo = Arc::new(MockNotificationRepo::new());
        let service =
            NotificationService::with_repository(NotificationServiceConfig::default(), repo);

        let subscribed_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let unsubscribed_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let channel_1 = Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: "channel-1".to_string(),
            db_channel_id: Some("channel-1".to_string()),
            display_name: "Channel 1".to_string(),
            channel_type: "WEBHOOK".to_string(),
            channel: Arc::new(TestChannel {
                channel_type: "subscribed",
                fail_for_attempts: 0,
                attempts: subscribed_attempts.clone(),
            }),
        });
        service.registry.write().insert(channel_1);

        let channel_2 = Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: "channel-2".to_string(),
            db_channel_id: Some("channel-2".to_string()),
            display_name: "Channel 2".to_string(),
            channel_type: "WEBHOOK".to_string(),
            channel: Arc::new(TestChannel {
                channel_type: "unsubscribed",
                fail_for_attempts: 0,
                attempts: unsubscribed_attempts.clone(),
            }),
        });
        service.registry.write().insert(channel_2);

        service
            .registry
            .write()
            .subscriptions_by_event
            .insert("system_startup".to_string(), vec!["channel-1".to_string()]);

        service
            .notify(NotificationEvent::SystemStartup {
                version: "test".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(subscribed_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(unsubscribed_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dead_letters_persisted_to_repository() {
        let repo = Arc::new(MockNotificationRepo::new());
        let config = NotificationServiceConfig {
            max_retries: 1,
            initial_retry_delay_ms: 1,
            max_retry_delay_ms: 5,
            ..Default::default()
        };
        let service = NotificationService::with_repository(config, repo.clone());

        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let fail_channel = Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: "channel-1".to_string(),
            db_channel_id: Some("channel-1".to_string()),
            display_name: "Channel 1".to_string(),
            channel_type: "WEBHOOK".to_string(),
            channel: Arc::new(TestChannel {
                channel_type: "fail",
                fail_for_attempts: 1,
                attempts: attempts.clone(),
            }),
        });
        service.registry.write().insert(fail_channel);

        service
            .registry
            .write()
            .subscriptions_by_event
            .insert("system_startup".to_string(), vec!["channel-1".to_string()]);

        service
            .notify(NotificationEvent::SystemStartup {
                version: "test".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let persisted = repo.dead_letters.lock().await;
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].channel_id, "channel-1");
        assert_eq!(persisted[0].event_name, "system_startup");
        assert_eq!(persisted[0].retry_count, 1);
        assert_eq!(service.stats().dead_letter_count, 1);
        assert_eq!(service.stats().pending_count, 0);
    }

    async fn assert_exhausted_channel_retained(with_repo: bool, with_id: bool, fail_insert: bool) {
        let repo = Arc::new(MockNotificationRepo {
            fail_dead_letter_insert: fail_insert,
            ..MockNotificationRepo::new()
        });
        let config = NotificationServiceConfig {
            max_retries: 2,
            initial_retry_delay_ms: 1,
            max_retry_delay_ms: 1,
            circuit_breaker_threshold: 100,
            ..Default::default()
        };
        let service = if with_repo {
            NotificationService::with_repository(config, repo.clone())
        } else {
            NotificationService::with_config(config)
        };
        let failed_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let successful_attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        for (key, attempts, fail_for_attempts) in [
            ("failed", failed_attempts.clone(), u32::MAX),
            ("successful", successful_attempts.clone(), 0),
        ] {
            let channel = Arc::new(RuntimeChannel {
                breaker: service.new_breaker(),
                key: key.into(),
                db_channel_id: with_id.then(|| key.to_string()),
                display_name: key.into(),
                channel_type: "test".into(),
                channel: Arc::new(TestChannel {
                    channel_type: "test",
                    fail_for_attempts,
                    attempts,
                }),
            });
            service.registry.write().insert(channel);
        }
        let event = NotificationEvent::SystemStartup {
            version: "test".into(),
            timestamp: Utc::now(),
        };
        service.dead_letters.insert(
            0,
            DeadLetterEntry {
                id: 0,
                notification_id: 0,
                event: event.clone(),
                channel_key: Some("old".into()),
                channel_id: None,
                channel_type: "test".into(),
                attempts: 2,
                error: "old failure".into(),
                created_at: Utc::now() - chrono::Duration::days(9),
                dead_lettered_at: Utc::now() - chrono::Duration::days(8),
            },
        );
        service
            .notify_channel_instances(
                ["failed".to_string(), "successful".to_string()]
                    .into_iter()
                    .collect(),
                event,
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while service.stats().pending_count != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retry delivery should settle");

        assert_eq!(failed_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(successful_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(service.stats().dead_letter_count, 1);
        assert!(
            !service.dead_letters.contains_key(&0),
            "retention removes old entries"
        );
        let entry = service.dead_letters.iter().next().unwrap();
        assert_eq!(entry.channel_key.as_deref(), Some("failed"));
        assert_eq!(entry.channel_id.as_deref(), with_id.then_some("failed"));
        assert_eq!(entry.attempts, 2);
        assert_eq!(entry.error, "forced failure 2");
        assert_eq!(
            repo.dead_letters.lock().await.len(),
            usize::from(with_repo && with_id && !fail_insert)
        );
    }

    #[tokio::test]
    async fn exhausted_config_channel_retained_without_repository() {
        assert_exhausted_channel_retained(false, false, false).await;
    }

    #[tokio::test]
    async fn exhausted_config_channel_retained_with_repository() {
        assert_exhausted_channel_retained(true, false, false).await;
    }

    #[tokio::test]
    async fn exhausted_channel_retained_when_persistence_fails() {
        assert_exhausted_channel_retained(true, true, true).await;
    }

    #[tokio::test]
    async fn exhausted_channel_retained_and_persisted_after_retries() {
        assert_exhausted_channel_retained(true, true, false).await;
    }

    #[tokio::test]
    async fn reload_from_db_normalizes_and_migrates_subscription_names() {
        let repo = Arc::new(MockNotificationRepo::new());
        let service = NotificationService::with_repository(
            NotificationServiceConfig::default(),
            repo.clone(),
        );

        let db_channel = NotificationChannelDbModel {
            id: "channel-1".to_string(),
            name: "Channel 1".to_string(),
            channel_type: "Webhook".to_string(),
            settings: r#"{"enabled":true,"url":"http://example.invalid","method":"POST"}"#
                .to_string(),
        };

        repo.channels.lock().await.push(db_channel);
        repo.subscriptions.lock().await.insert(
            "channel-1".to_string(),
            vec!["SystemStartup".to_string(), "download.complete".to_string()],
        );

        service.reload_from_db().await.unwrap();

        {
            let registry = service.registry.read();
            let subs = &registry.subscriptions_by_event;
            assert_eq!(
                subs.get("system_startup").cloned(),
                Some(vec!["channel-1".to_string()])
            );
            assert_eq!(
                subs.get("download_completed").cloned(),
                Some(vec!["channel-1".to_string()])
            );
        }

        let subscribe_calls = repo.subscribe_calls.lock().await.clone();
        assert!(
            subscribe_calls
                .iter()
                .any(|(c, e)| c == "channel-1" && e == "system_startup")
        );
        assert!(
            subscribe_calls
                .iter()
                .any(|(c, e)| c == "channel-1" && e == "download_completed")
        );

        let unsubscribe_calls = repo.unsubscribe_calls.lock().await.clone();
        assert!(
            unsubscribe_calls
                .iter()
                .any(|(c, e)| c == "channel-1" && e == "SystemStartup")
        );
        assert!(
            unsubscribe_calls
                .iter()
                .any(|(c, e)| c == "channel-1" && e == "download.complete")
        );
    }
}
