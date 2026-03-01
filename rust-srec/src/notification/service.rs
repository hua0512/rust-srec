//! Notification service implementation.
//!
//! The NotificationService is responsible for:
//! - Listening to system events (Monitor, Download, Pipeline)
//! - Dispatching notifications to configured channels
//! - Managing retry logic with exponential backoff
//! - Implementing circuit breaker pattern for failing channels
//! - Maintaining a dead letter queue for failed notifications

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::channels::{
    ChannelConfig, DiscordChannel, EmailChannel, NotificationChannel, TelegramChannel,
    WebhookChannel,
};
use super::events::canonicalize_subscription_event_name;
use super::events::{NotificationEvent, NotificationPriority};
use super::web_push::WebPushService;
use crate::Result;
use crate::database::models::{
    ChannelType, DiscordChannelSettings, EmailChannelSettings, NotificationChannelDbModel,
    NotificationDeadLetterDbModel, NotificationEventLogDbModel, TelegramChannelSettings,
    WebhookChannelSettings,
};
use crate::database::repositories::NotificationRepository;
use crate::downloader::DownloadManagerEvent;
use crate::monitor::MonitorEvent;
use crate::pipeline::PipelineEvent;

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
    /// Maximum queue size for pending notifications.
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
            if elapsed.num_seconds() as u64 >= self.cooldown.as_secs() {
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

#[derive(Clone)]
struct RuntimeChannel {
    key: String,
    db_channel_id: Option<String>,
    display_name: String,
    channel_type: String,
    channel: Arc<dyn NotificationChannel>,
}

/// A notification pending delivery.
#[derive(Debug, Clone)]
struct PendingNotification {
    _id: u64,
    event: NotificationEvent,
    created_at: DateTime<Utc>,
    channel_state: HashMap<String, ChannelDeliveryState>,
    retry_generation: u64,
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

struct RetryParams {
    id: u64,
    delay: Duration,
    expected_generation: u64,
    channels: Vec<Arc<RuntimeChannel>>,
    pending_queue: Arc<DashMap<u64, PendingNotification>>,
    dead_letters: Arc<DashMap<u64, DeadLetterEntry>>,
    dead_letter_cleanup_ts: Arc<AtomicU64>,
    circuit_breakers: Arc<DashMap<String, CircuitBreakerState>>,
    notification_repo: Option<Arc<dyn NotificationRepository>>,
    config: NotificationServiceConfig,
    next_dead_letter_id: Arc<AtomicU64>,
    cancellation_token: CancellationToken,
}

struct ProcessingParams {
    id: u64,
    channels: Vec<Arc<RuntimeChannel>>,
    pending_queue: Arc<DashMap<u64, PendingNotification>>,
    dead_letters: Arc<DashMap<u64, DeadLetterEntry>>,
    dead_letter_cleanup_ts: Arc<AtomicU64>,
    circuit_breakers: Arc<DashMap<String, CircuitBreakerState>>,
    notification_repo: Option<Arc<dyn NotificationRepository>>,
    config: NotificationServiceConfig,
    next_dead_letter_id: Arc<AtomicU64>,
    cancellation_token: CancellationToken,
}

/// The notification service.
pub struct NotificationService {
    config: NotificationServiceConfig,
    notification_repo: Option<Arc<dyn NotificationRepository>>,
    web_push_service: Option<Arc<WebPushService>>,
    web_push_tx: parking_lot::RwLock<Option<mpsc::Sender<WebPushQueuedEvent>>>,
    web_push_worker_started: AtomicBool,
    web_push_worker_handle: parking_lot::RwLock<Option<JoinHandle<()>>>,
    subscriptions_by_event: RwLock<HashMap<String, Vec<String>>>,
    channels: RwLock<Vec<Arc<RuntimeChannel>>>,
    channels_by_key: DashMap<String, Arc<RuntimeChannel>>,
    circuit_breakers: Arc<DashMap<String, CircuitBreakerState>>,
    pending_queue: Arc<DashMap<u64, PendingNotification>>,
    dead_letters: Arc<DashMap<u64, DeadLetterEntry>>,
    /// Last time we performed in-memory dead-letter retention cleanup (unix epoch seconds).
    dead_letter_cleanup_ts: Arc<AtomicU64>,
    next_id: AtomicU64,
    next_dead_letter_id: Arc<AtomicU64>,
    event_tx: broadcast::Sender<NotificationEvent>,
    cancellation_token: CancellationToken,
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

    /// Create a new notification service with custom configuration.
    pub fn with_config(config: NotificationServiceConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        let service = Self {
            notification_repo: None,
            web_push_service: None,
            web_push_tx: parking_lot::RwLock::new(None),
            web_push_worker_started: AtomicBool::new(false),
            web_push_worker_handle: parking_lot::RwLock::new(None),
            subscriptions_by_event: RwLock::new(HashMap::new()),
            channels: RwLock::new(Vec::new()),
            channels_by_key: DashMap::new(),
            circuit_breakers: Arc::new(DashMap::new()),
            pending_queue: Arc::new(DashMap::new()),
            dead_letters: Arc::new(DashMap::new()),
            dead_letter_cleanup_ts: Arc::new(AtomicU64::new(0)),
            next_id: AtomicU64::new(1),
            next_dead_letter_id: Arc::new(AtomicU64::new(1)),
            event_tx,
            cancellation_token: CancellationToken::new(),
            config,
        };

        // Initialize channels from config
        service.init_channels();

        service
    }

    /// Start a background worker to process web push delivery from a bounded queue.
    ///
    /// This avoids spawning a new task per notification event and reduces DB load by
    /// letting `WebPushService` reuse its internal subscription cache.
    pub fn start_web_push_worker(self: &Arc<Self>) {
        if self.web_push_service.is_none() {
            return;
        }
        if self
            .web_push_worker_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let (tx, mut rx) = mpsc::channel::<WebPushQueuedEvent>(WEB_PUSH_QUEUE_CAPACITY);
        *self.web_push_tx.write() = Some(tx);

        let service = Arc::clone(self);
        let web_push = service
            .web_push_service
            .as_ref()
            .expect("web_push_service checked above")
            .clone();
        let cancellation_token = service.cancellation_token.clone();

        let handle = tokio::spawn(async move {
            async fn flush_buffer(
                web_push: &Arc<WebPushService>,
                buffer: &mut Vec<WebPushQueuedEvent>,
            ) {
                let mut batch = std::mem::take(buffer);
                for item in &batch {
                    web_push
                        .send_event(&item.event, item.event_log_id.as_deref())
                        .await;
                }
                batch.clear();
                *buffer = batch;
            }

            let mut buffer: Vec<WebPushQueuedEvent> = Vec::with_capacity(WEB_PUSH_BATCH_SIZE);
            let mut ticker =
                tokio::time::interval(Duration::from_millis(WEB_PUSH_FLUSH_INTERVAL_MS));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Web push worker shutting down");
                        while let Ok(item) = rx.try_recv() {
                            buffer.push(item);
                        }
                        if !buffer.is_empty() {
                            flush_buffer(&web_push, &mut buffer).await;
                        }
                        break;
                    }
                    _ = ticker.tick() => {
                        if buffer.is_empty() {
                            continue;
                        }
                        flush_buffer(&web_push, &mut buffer).await;
                    }
                    maybe = rx.recv() => {
                        let Some(item) = maybe else {
                            while let Ok(item) = rx.try_recv() {
                                buffer.push(item);
                            }
                            if !buffer.is_empty() {
                                flush_buffer(&web_push, &mut buffer).await;
                            }
                            break;
                        };
                        buffer.push(item);
                        if buffer.len() < WEB_PUSH_BATCH_SIZE {
                            continue;
                        }
                        flush_buffer(&web_push, &mut buffer).await;
                    }
                }
            }
        });

        *service.web_push_worker_handle.write() = Some(handle);
    }

    /// Initialize channels from configuration.
    fn init_channels(&self) {
        let mut channels = self.channels.write();
        channels.clear();
        self.channels_by_key.clear();
        let mut used_keys: HashSet<String> = HashSet::new();

        for (idx, channel_config) in self.config.channels.iter().enumerate() {
            let channel: Arc<dyn NotificationChannel> = match channel_config {
                ChannelConfig::Discord(c) => Arc::new(DiscordChannel::new(c.clone())),
                ChannelConfig::Email(c) => Arc::new(EmailChannel::new(c.clone())),
                ChannelConfig::Telegram(c) => Arc::new(TelegramChannel::new(c.clone())),
                ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
            };

            if channel.is_enabled() {
                let base_key = match channel_config.instance_id() {
                    Some(id) => format!(
                        "config:{}:{}",
                        channel_config.channel_type(),
                        normalize_channel_key_part(id)
                    ),
                    None => format!("config:{}:{}", channel_config.channel_type(), idx),
                };
                let key = if used_keys.insert(base_key.clone()) {
                    base_key
                } else {
                    let disambiguated = format!("{}:{}", base_key, idx);
                    warn!(
                        "Duplicate notification channel key detected (base_key={}), using {}",
                        base_key, disambiguated
                    );
                    used_keys.insert(disambiguated.clone());
                    disambiguated
                };
                let display_name = channel_config
                    .display_name()
                    .unwrap_or(channel_config.channel_type())
                    .to_string();

                self.circuit_breakers.insert(
                    key.clone(),
                    CircuitBreakerState::new(self.config.circuit_breaker_cooldown_secs),
                );

                let runtime = Arc::new(RuntimeChannel {
                    key,
                    db_channel_id: None,
                    display_name,
                    channel_type: channel.channel_type().to_string(),
                    channel,
                });
                self.channels_by_key
                    .insert(runtime.key.clone(), runtime.clone());
                channels.push(runtime);
                info!(
                    "Initialized notification channel: {}",
                    channel_config.channel_type()
                );
            }
        }

        info!(
            "Notification service initialized with {} channels",
            channels.len()
        );
    }

    /// Add a channel dynamically.
    pub fn add_channel(&self, config: ChannelConfig) {
        let channel: Arc<dyn NotificationChannel> = match &config {
            ChannelConfig::Discord(c) => Arc::new(DiscordChannel::new(c.clone())),
            ChannelConfig::Email(c) => Arc::new(EmailChannel::new(c.clone())),
            ChannelConfig::Telegram(c) => Arc::new(TelegramChannel::new(c.clone())),
            ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
        };

        if channel.is_enabled() {
            let key = format!("dynamic:{}", Uuid::new_v4());
            let display_name = config
                .display_name()
                .unwrap_or(config.channel_type())
                .to_string();
            self.circuit_breakers.insert(
                key.clone(),
                CircuitBreakerState::new(self.config.circuit_breaker_cooldown_secs),
            );
            let runtime = Arc::new(RuntimeChannel {
                key,
                db_channel_id: None,
                display_name,
                channel_type: channel.channel_type().to_string(),
                channel,
            });
            self.channels_by_key
                .insert(runtime.key.clone(), runtime.clone());
            self.channels.write().push(runtime);
            info!("Added notification channel: {}", config.channel_type());
        }
    }

    pub async fn reload_from_db(&self) -> Result<()> {
        let Some(repo) = self.notification_repo.as_ref().cloned() else {
            return Ok(());
        };

        let db_channels = repo.list_channels().await?;

        let existing_config_channels: Vec<Arc<RuntimeChannel>> = self
            .channels
            .read()
            .iter()
            .filter(|c| c.db_channel_id.is_none())
            .cloned()
            .collect();

        let mut new_db_channels = Vec::new();
        let mut subscriptions_by_event: HashMap<String, Vec<String>> = HashMap::new();

        for db_channel in db_channels {
            let runtime_channel = match self.build_runtime_channel_from_db(&db_channel) {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        "Skipping invalid notification channel id={} type={}: {}",
                        db_channel.id, db_channel.channel_type, e
                    );
                    continue;
                }
            };

            self.circuit_breakers.insert(
                runtime_channel.key.clone(),
                CircuitBreakerState::new(self.config.circuit_breaker_cooldown_secs),
            );

            let subscriptions = repo.get_subscriptions_for_channel(&db_channel.id).await?;
            for raw_event_name in subscriptions {
                let Some(canonical) = canonicalize_subscription_event_name(&raw_event_name) else {
                    warn!(
                        "Skipping unknown notification subscription for channel {}: {}",
                        db_channel.id, raw_event_name
                    );
                    continue;
                };

                subscriptions_by_event
                    .entry(canonical.to_string())
                    .or_default()
                    .push(db_channel.id.clone());

                // Best-effort migration toward canonical event names.
                if raw_event_name.trim() != canonical {
                    if let Err(e) = repo.subscribe(&db_channel.id, canonical).await {
                        warn!(
                            "Failed to migrate notification subscription (channel={}, from={}, to={}): {}",
                            db_channel.id, raw_event_name, canonical, e
                        );
                    }
                    if let Err(e) = repo.unsubscribe(&db_channel.id, &raw_event_name).await {
                        warn!(
                            "Failed to remove legacy notification subscription (channel={}, event={}): {}",
                            db_channel.id, raw_event_name, e
                        );
                    }
                }
            }

            new_db_channels.push(runtime_channel);
        }

        let mut combined_channels = existing_config_channels;
        combined_channels.extend(new_db_channels);

        let live_keys: HashSet<String> = combined_channels.iter().map(|c| c.key.clone()).collect();
        self.circuit_breakers.retain(|k, _| live_keys.contains(k));

        *self.subscriptions_by_event.write() = subscriptions_by_event;
        *self.channels.write() = combined_channels;
        self.channels_by_key.clear();
        for channel in self.channels.read().iter() {
            self.channels_by_key
                .insert(channel.key.clone(), channel.clone());
        }

        // Prevent stuck pending notifications if channels were removed/renamed.
        // Any pending delivery state keyed to a non-existent channel is dropped.
        let pending_ids: Vec<u64> = self.pending_queue.iter().map(|e| *e.key()).collect();
        for id in pending_ids {
            if let Some(mut pending) = self.pending_queue.get_mut(&id) {
                pending.channel_state.retain(|k, _| live_keys.contains(k));
                if pending.channel_state.is_empty() {
                    drop(pending);
                    self.pending_queue.remove(&id);
                }
            }
        }

        info!(
            "Notification DB config loaded: channels={}, subscribed_events={}",
            self.channels.read().len(),
            self.subscriptions_by_event.read().len()
        );

        Ok(())
    }

    fn build_runtime_channel_from_db(
        &self,
        db_channel: &NotificationChannelDbModel,
    ) -> Result<Option<Arc<RuntimeChannel>>> {
        let settings_json: Value = serde_json::from_str(&db_channel.settings)?;
        let enabled = settings_json
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            return Ok(None);
        }

        let channel_type = ChannelType::parse(&db_channel.channel_type).ok_or_else(|| {
            crate::Error::Validation(format!(
                "Unsupported notification channel_type: {}",
                db_channel.channel_type
            ))
        })?;

        let min_priority = settings_json
            .get("min_priority")
            .and_then(|v| v.as_str())
            .and_then(parse_notification_priority)
            .unwrap_or(NotificationPriority::Normal);

        let runtime_channel: Arc<dyn NotificationChannel> = match channel_type {
            ChannelType::Discord => {
                let settings: DiscordChannelSettings =
                    serde_json::from_value(settings_json.clone())?;
                Arc::new(DiscordChannel::new(super::channels::DiscordConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    webhook_url: settings.webhook_url,
                    username: settings.username,
                    avatar_url: settings.avatar_url,
                    min_priority,
                }))
            }
            ChannelType::Email => {
                let settings: EmailChannelSettings = serde_json::from_value(settings_json.clone())?;
                Arc::new(EmailChannel::new(super::channels::EmailConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    smtp_host: settings.smtp_host,
                    smtp_port: settings.smtp_port,
                    smtp_username: if settings.username.is_empty() {
                        None
                    } else {
                        Some(settings.username)
                    },
                    smtp_password: if settings.password.is_empty() {
                        None
                    } else {
                        Some(settings.password)
                    },
                    use_tls: settings.use_tls,
                    from_address: settings.from_address,
                    to_addresses: settings.to_addresses,
                    min_priority,
                    batch_window_secs: settings_json
                        .get("batch_window_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(60),
                }))
            }
            ChannelType::Telegram => {
                let settings: TelegramChannelSettings =
                    serde_json::from_value(settings_json.clone())?;
                Arc::new(TelegramChannel::new(super::channels::TelegramConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    bot_token: settings.bot_token,
                    chat_id: settings.chat_id,
                    parse_mode: settings_json
                        .get("parse_mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("HTML")
                        .to_string(),
                    min_priority,
                }))
            }
            ChannelType::Webhook => {
                let settings: WebhookChannelSettings =
                    serde_json::from_value(settings_json.clone())?;

                let mut headers_vec = Vec::new();
                if let Some(headers) = settings.headers {
                    let mut keys: Vec<_> = headers.keys().cloned().collect();
                    keys.sort();
                    for k in keys {
                        if let Some(v) = headers.get(&k) {
                            headers_vec.push((k.clone(), v.clone()));
                        }
                    }
                }

                let auth = if let Some(auth_val) = settings.auth {
                    match serde_json::from_value::<super::channels::WebhookAuth>(auth_val.clone()) {
                        Ok(a) => Some(a),
                        Err(e) => {
                            warn!("Failed to parse webhook auth for channel: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                Arc::new(WebhookChannel::new(super::channels::WebhookConfig {
                    id: None,
                    name: None,
                    enabled: settings.enabled.unwrap_or(true),
                    url: settings.url,
                    method: settings.method,
                    headers: headers_vec,
                    auth,
                    min_priority,
                    timeout_secs: settings.timeout_secs.unwrap_or(30),
                }))
            }
        };

        if !runtime_channel.is_enabled() {
            return Ok(None);
        }

        Ok(Some(Arc::new(RuntimeChannel {
            key: db_channel.id.clone(),
            db_channel_id: Some(db_channel.id.clone()),
            display_name: db_channel.name.clone(),
            channel_type: db_channel.channel_type.clone(),
            channel: runtime_channel,
        })))
    }

    /// Subscribe to notification events.
    pub fn subscribe(&self) -> broadcast::Receiver<NotificationEvent> {
        self.event_tx.subscribe()
    }

    /// List currently loaded channel instances (config + dynamic + DB).
    pub fn list_channel_instances(&self) -> Vec<NotificationChannelInstance> {
        self.channels
            .read()
            .iter()
            .map(|c| NotificationChannelInstance {
                key: c.key.clone(),
                channel_id: c.db_channel_id.clone(),
                display_name: c.display_name.clone(),
                channel_type: c.channel_type.clone(),
                source: if c.db_channel_id.is_some() {
                    NotificationChannelSource::Database
                } else if c.key.starts_with("dynamic:") {
                    NotificationChannelSource::Dynamic
                } else {
                    NotificationChannelSource::Config
                },
            })
            .collect()
    }

    /// Run a connectivity/config test for a specific channel instance.
    pub async fn test_channel_instance(&self, key: &str) -> Result<()> {
        let channel = self
            .channels_by_key
            .get(key)
            .map(|c| c.clone())
            .ok_or_else(|| crate::Error::NotFound {
                entity_type: "NotificationChannelInstance".to_string(),
                id: key.to_string(),
            })?;
        channel.channel.test().await
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
        let mut missing_keys = Vec::new();
        for key in &keys {
            if !self.channels_by_key.contains_key(key) {
                missing_keys.push(key.clone());
            }
        }
        if let Some(missing_key) = missing_keys.first() {
            return Err(crate::Error::NotFound {
                entity_type: "NotificationChannelInstance".to_string(),
                id: missing_key.clone(),
            });
        }

        let target_channels: Vec<Arc<RuntimeChannel>> = keys
            .iter()
            .filter_map(|k| self.channels_by_key.get(k).map(|c| c.clone()))
            .collect();

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
            retry_generation: 0,
            next_retry_at: None,
        };

        if self.pending_queue.len() >= self.config.max_queue_size {
            warn!("Notification queue full, dropping oldest notification");
            if let Some(oldest) = self.pending_queue.iter().min_by_key(|e| e.created_at) {
                let oldest_id = *oldest.key();
                drop(oldest);
                self.pending_queue.remove(&oldest_id);
            }
        }

        self.pending_queue.insert(id, pending);
        self.process_notification(id).await;
        Ok(())
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
                priority: event.priority().to_string(),
                payload: serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string()),
                streamer_id: event.streamer_id().map(|s| s.to_string()),
                created_at: event.timestamp().timestamp_millis(),
            };
            if let Err(e) = repo.add_event_log(&entry).await {
                warn!(error = %e, "Failed to persist notification event log (non-fatal)");
            }
        }

        // Best-effort: send web push notifications (independent of channel delivery).
        if let Some(web_push) = self.web_push_service.as_ref().cloned() {
            let queued = WebPushQueuedEvent {
                event: event.clone(),
                event_log_id: Some(event_log_id.clone()),
            };

            let send_result = self
                .web_push_tx
                .read()
                .clone()
                .map(|tx| tx.try_send(queued));

            match send_result {
                Some(Ok(())) => {}
                Some(Err(err)) => {
                    warn!(error = %err, "Web push queue full/closed; falling back to detached send");
                    let event = event.clone();
                    let event_log_id = event_log_id.clone();
                    tokio::spawn(async move {
                        web_push.send_event(&event, Some(&event_log_id)).await;
                    });
                }
                None => {
                    let event = event.clone();
                    let event_log_id = event_log_id.clone();
                    tokio::spawn(async move {
                        web_push.send_event(&event, Some(&event_log_id)).await;
                    });
                }
            }
        }

        // Queue the notification
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let channels = self.channels.read().clone();
        if channels.is_empty() {
            return Ok(());
        }

        let subscribed_db_channel_ids = {
            let subscriptions = self.subscriptions_by_event.read();
            let mut ids: HashSet<String> = HashSet::new();
            if let Some(channels) = subscriptions.get(event.event_type()) {
                ids.extend(channels.iter().cloned());
            }
            ids
        };

        let target_channels: Vec<Arc<RuntimeChannel>> = channels
            .into_iter()
            .filter(|c| match &c.db_channel_id {
                None => true, // config/dynamic channels always receive events
                Some(db_id) => subscribed_db_channel_ids.contains(db_id),
            })
            .collect();

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
            retry_generation: 0,
            next_retry_at: None,
        };

        // Check queue size
        if self.pending_queue.len() >= self.config.max_queue_size {
            warn!("Notification queue full, dropping oldest notification");
            // Remove oldest notification
            if let Some(oldest) = self.pending_queue.iter().min_by_key(|e| e.created_at) {
                let oldest_id = *oldest.key();
                drop(oldest);
                self.pending_queue.remove(&oldest_id);
            }
        }

        self.pending_queue.insert(id, pending);

        // Process immediately
        self.process_notification(id).await;

        Ok(())
    }

    /// Process a pending notification.
    async fn process_notification(&self, id: u64) {
        let channels = self.channels.read().clone();
        Self::process_notification_detached(ProcessingParams {
            id,
            channels,
            pending_queue: self.pending_queue.clone(),
            dead_letters: self.dead_letters.clone(),
            dead_letter_cleanup_ts: self.dead_letter_cleanup_ts.clone(),
            circuit_breakers: self.circuit_breakers.clone(),
            notification_repo: self.notification_repo.clone(),
            config: self.config.clone(),
            next_dead_letter_id: self.next_dead_letter_id.clone(),
            cancellation_token: self.cancellation_token.clone(),
        })
        .await;
    }

    fn maybe_cleanup_dead_letters_detached(
        dead_letters: &DashMap<u64, DeadLetterEntry>,
        retention_days: u32,
        dead_letter_cleanup_ts: &AtomicU64,
        now: DateTime<Utc>,
    ) {
        let now_ts = now.timestamp().max(0) as u64;
        let last = dead_letter_cleanup_ts.load(Ordering::Relaxed);
        if now_ts.saturating_sub(last) < DEAD_LETTER_CLEANUP_INTERVAL_SECS {
            return;
        }
        if dead_letter_cleanup_ts
            .compare_exchange(last, now_ts, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        if retention_days == 0 {
            dead_letters.clear();
            return;
        }

        let retention = chrono::Duration::days(retention_days as i64);
        let cutoff = now - retention;
        dead_letters.retain(|_, entry| entry.dead_lettered_at > cutoff);
    }

    /// Calculate retry delay with exponential backoff and jitter.
    fn _calculate_retry_delay(&self, attempts: u32) -> Duration {
        let base_delay = self.config.initial_retry_delay_ms;
        let max_delay = self.config.max_retry_delay_ms;

        // Exponential backoff: delay = base * 2^attempts
        let delay_ms = base_delay.saturating_mul(2u64.saturating_pow(attempts));
        let delay_ms = delay_ms.min(max_delay);

        // Add jitter (±25%)
        let jitter_range = delay_ms / 4;
        let jitter = if jitter_range > 0 {
            (rand::random::<u64>() % (jitter_range * 2)).saturating_sub(jitter_range)
        } else {
            0
        };

        Duration::from_millis(delay_ms.saturating_add(jitter))
    }

    fn spawn_retry_detached(params: RetryParams) {
        let RetryParams {
            id,
            delay,
            expected_generation,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
        } = params;
        debug!(
            "Scheduling retry for notification {} in {:?} (gen={})",
            id, delay, expected_generation
        );

        tokio::spawn(async move {
            tokio::select! {
                _ = cancellation_token.cancelled() => return,
                _ = sleep(delay) => {},
            }

            let should_run = pending_queue
                .get(&id)
                .map(|p| p.retry_generation == expected_generation)
                .unwrap_or(false);
            if !should_run {
                return;
            }

            Self::process_notification_detached(ProcessingParams {
                id,
                channels,
                pending_queue,
                dead_letters,
                dead_letter_cleanup_ts,
                circuit_breakers,
                notification_repo,
                config,
                next_dead_letter_id,
                cancellation_token,
            })
            .await;
        });
    }

    fn calculate_retry_delay_detached(
        config: &NotificationServiceConfig,
        attempts: u32,
    ) -> Duration {
        let base_delay = config.initial_retry_delay_ms;
        let max_delay = config.max_retry_delay_ms;

        let delay_ms = base_delay.saturating_mul(2u64.saturating_pow(attempts));
        let delay_ms = delay_ms.min(max_delay);

        let jitter_range = delay_ms / 4;
        let jitter = if jitter_range > 0 {
            (rand::random::<u64>() % (jitter_range * 2)).saturating_sub(jitter_range)
        } else {
            0
        };

        Duration::from_millis(delay_ms.saturating_add(jitter))
    }

    async fn process_notification_detached(params: ProcessingParams) {
        let ProcessingParams {
            id,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
        } = params;
        let pending_snapshot = match pending_queue.get(&id) {
            Some(p) => p.clone(),
            None => return,
        };

        let mut circuit_blocked = false;

        for channel in &channels {
            let channel_key = channel.key.clone();

            let is_pending = pending_queue
                .get(&id)
                .and_then(|p| p.channel_state.get(&channel_key).map(|cs| cs.status))
                == Some(DeliveryStatus::Pending);
            if !is_pending {
                continue;
            }

            let allowed = circuit_breakers
                .get(&channel_key)
                .map(|cb| cb.is_allowed())
                .unwrap_or(true);
            if !allowed {
                circuit_blocked = true;
                continue;
            }

            let event = pending_snapshot.event.clone();
            match channel.channel.send(&event).await {
                Ok(()) => {
                    if let Some(mut cb) = circuit_breakers.get_mut(&channel_key) {
                        cb.record_success();
                    }
                    if let Some(mut p) = pending_queue.get_mut(&id)
                        && let Some(cs) = p.channel_state.get_mut(&channel_key)
                    {
                        cs.status = DeliveryStatus::Delivered;
                        cs.last_attempt = Some(Utc::now());
                        cs.last_error = None;
                    }
                    debug!("Notification {} sent via {}", id, channel.channel_type);
                }
                Err(e) => {
                    if let Some(mut cb) = circuit_breakers.get_mut(&channel_key) {
                        cb.record_failure(config.circuit_breaker_threshold);
                    }

                    let now = Utc::now();
                    let mut attempts = 0;
                    let mut dead_lettered = false;

                    if let Some(mut p) = pending_queue.get_mut(&id)
                        && let Some(cs) = p.channel_state.get_mut(&channel_key)
                    {
                        cs.attempts += 1;
                        cs.last_attempt = Some(now);
                        cs.last_error = Some(e.to_string());
                        attempts = cs.attempts;
                        if cs.attempts >= config.max_retries {
                            cs.status = DeliveryStatus::DeadLettered;
                            dead_lettered = true;
                        }
                    }

                    if dead_lettered
                        && let (Some(repo), Some(db_channel_id)) =
                            (notification_repo.clone(), channel.db_channel_id.clone())
                    {
                        if let Ok(payload) = serde_json::to_string(&pending_snapshot.event) {
                            let db_entry = NotificationDeadLetterDbModel::new(
                                db_channel_id.clone(),
                                pending_snapshot.event.event_type(),
                                payload,
                                e.to_string(),
                                attempts as i32,
                                pending_snapshot.created_at.timestamp_millis(),
                            );
                            if let Err(err) = repo.add_to_dead_letter(&db_entry).await {
                                warn!(
                                    "Failed to persist dead letter entry for channel {}: {}",
                                    db_channel_id, err
                                );
                            }
                        }

                        let dead_letter_id = next_dead_letter_id.fetch_add(1, Ordering::SeqCst);
                        dead_letters.insert(
                            dead_letter_id,
                            DeadLetterEntry {
                                id: dead_letter_id,
                                notification_id: id,
                                event: pending_snapshot.event.clone(),
                                channel_key: Some(channel_key.clone()),
                                channel_id: channel.db_channel_id.clone(),
                                channel_type: channel.channel_type.clone(),
                                attempts,
                                error: e.to_string(),
                                created_at: pending_snapshot.created_at,
                                dead_lettered_at: now,
                            },
                        );

                        Self::maybe_cleanup_dead_letters_detached(
                            &dead_letters,
                            config.dead_letter_retention_days,
                            &dead_letter_cleanup_ts,
                            now,
                        );
                        warn!(
                            "Notification {} dead-lettered for channel {} after {} attempts",
                            id, channel.channel_type, config.max_retries
                        );
                    }
                }
            }
        }

        let (has_pending, min_delay) = match pending_queue.get(&id) {
            Some(p) => {
                let mut min_delay: Option<Duration> = None;
                let mut has_pending = false;
                for cs in p.channel_state.values() {
                    if cs.status != DeliveryStatus::Pending {
                        continue;
                    }
                    has_pending = true;
                    let delay = Self::calculate_retry_delay_detached(&config, cs.attempts);
                    min_delay = Some(match min_delay {
                        Some(existing) => existing.min(delay),
                        None => delay,
                    });
                }
                (has_pending, min_delay)
            }
            None => return,
        };

        if !has_pending {
            pending_queue.remove(&id);
            return;
        }

        let mut delay = min_delay.unwrap_or_else(|| Duration::from_secs(1));
        if circuit_blocked {
            delay = delay.max(Duration::from_secs(config.circuit_breaker_cooldown_secs));
        }

        let expected_generation = match pending_queue.get_mut(&id) {
            Some(mut p) => {
                p.retry_generation = p.retry_generation.saturating_add(1);
                p.next_retry_at = Some(
                    Utc::now()
                        + chrono::Duration::from_std(delay)
                            .unwrap_or_else(|_| chrono::Duration::seconds(delay.as_secs() as i64)),
                );
                p.retry_generation
            }
            None => return,
        };

        Self::spawn_retry_detached(RetryParams {
            id,
            delay,
            expected_generation,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
        });
    }

    /// Get dead letter entries.
    pub fn get_dead_letters(&self) -> Vec<DeadLetterEntry> {
        self.cleanup_dead_letters();
        self.dead_letters
            .iter()
            .map(|e| e.value().clone())
            .collect()
    }

    /// Retry a dead letter notification.
    pub async fn retry_dead_letter(&self, id: u64) -> Result<()> {
        if let Some((_, dead_letter)) = self.dead_letters.remove(&id) {
            if let Some(channel_key) = dead_letter.channel_key.clone() {
                self.notify_channel_instance(&channel_key, dead_letter.event)
                    .await
            } else {
                self.notify(dead_letter.event).await
            }
        } else {
            Err(crate::Error::NotFound {
                entity_type: "DeadLetter".to_string(),
                id: id.to_string(),
            })
        }
    }

    /// Clear old dead letters.
    pub fn cleanup_dead_letters(&self) {
        let now = Utc::now();
        self.dead_letter_cleanup_ts
            .store(now.timestamp().max(0) as u64, Ordering::Relaxed);

        if self.config.dead_letter_retention_days == 0 {
            self.dead_letters.clear();
            return;
        }

        let retention = chrono::Duration::days(self.config.dead_letter_retention_days as i64);
        let cutoff = now - retention;

        self.dead_letters
            .retain(|_, entry| entry.dead_lettered_at > cutoff);
    }

    /// Get queue statistics.
    pub fn stats(&self) -> NotificationStats {
        NotificationStats {
            pending_count: self.pending_queue.len(),
            dead_letter_count: self.dead_letters.len(),
            channel_count: self.channels.read().len(),
            circuit_breakers: self
                .circuit_breakers
                .iter()
                .map(|e| (e.key().clone(), e.is_open))
                .collect(),
        }
    }

    /// Start listening for system events.
    pub fn start_event_listeners(
        self: &Arc<Self>,
        monitor_rx: broadcast::Receiver<MonitorEvent>,
        download_rx: broadcast::Receiver<DownloadManagerEvent>,
        pipeline_rx: broadcast::Receiver<PipelineEvent>,
    ) {
        self.listen_for_monitor_events(monitor_rx);
        self.listen_for_download_events(download_rx);
        self.listen_for_pipeline_events(pipeline_rx);
    }

    /// Listen for monitor events.
    fn listen_for_monitor_events(self: &Arc<Self>, mut rx: broadcast::Receiver<MonitorEvent>) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Monitor event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                if !event.should_notify() {
                                    continue;
                                }

                                let notification = match event {
                                    MonitorEvent::StreamerLive {
                                        streamer_id,
                                        streamer_name,
                                        title,
                                        category,
                                        timestamp,
                                        ..
                                    } => Some(NotificationEvent::StreamOnline {
                                        streamer_id,
                                        streamer_name,
                                        title,
                                        category,
                                        timestamp,
                                    }),
                                    MonitorEvent::StreamerOffline {
                                        streamer_id,
                                        streamer_name,
                                        timestamp,
                                        ..
                                    } => Some(NotificationEvent::StreamOffline {
                                        streamer_id,
                                        streamer_name,
                                        duration_secs: None,
                                        timestamp,
                                    }),
                                    MonitorEvent::FatalError {
                                        streamer_id,
                                        streamer_name,
                                        error_type,
                                        message,
                                        timestamp,
                                        ..
                                    } => Some(NotificationEvent::FatalError {
                                        streamer_id,
                                        streamer_name,
                                        error_type: format!("{:?}", error_type),
                                        message,
                                        timestamp,
                                    }),
                                    _ => None,
                                };

                                if let Some(notification) = notification {
                                    let service = service.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = service.notify(notification).await {
                                            warn!("Failed to dispatch notification: {}", e);
                                        }
                                    });
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Monitor event listener lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Monitor event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Listen for download events.
    fn listen_for_download_events(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<DownloadManagerEvent>,
    ) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                let notification = match event {
                                    DownloadManagerEvent::DownloadStarted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        ..
                                    } => Some(NotificationEvent::DownloadStarted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadCompleted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        total_bytes,
                                        total_duration_secs,
                                        ..
                                    } => Some(NotificationEvent::DownloadCompleted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        file_size_bytes: total_bytes,
                                        duration_secs: total_duration_secs,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadFailed {
                                        streamer_id,
                                        streamer_name,
                                        error,
                                        recoverable,
                                        ..
                                    } => Some(NotificationEvent::DownloadError {
                                        streamer_id,
                                        streamer_name,
                                        error_message: error,
                                        recoverable,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::SegmentStarted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        segment_path,
                                        segment_index,
                                        ..
                                    } => Some(NotificationEvent::SegmentStarted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        segment_path,
                                        segment_index,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::SegmentCompleted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        segment_path,
                                        segment_index,
                                        duration_secs,
                                        size_bytes,
                                        ..
                                    } => Some(NotificationEvent::SegmentCompleted {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        segment_path,
                                        segment_index,
                                        size_bytes,
                                        duration_secs,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadCancelled {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        ..
                                    } => Some(NotificationEvent::DownloadCancelled {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadRejected {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        reason,
                                        ..
                                    } => Some(NotificationEvent::DownloadRejected {
                                        streamer_id,
                                        streamer_name,
                                        session_id,
                                        reason,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::ConfigUpdated {
                                        streamer_id,
                                        streamer_name,
                                        update_type,
                                        ..
                                    } => Some(NotificationEvent::ConfigUpdated {
                                        streamer_id,
                                        streamer_name,
                                        update_type: format!("{:?}", update_type),
                                        timestamp: Utc::now(),
                                    }),
                                    _ => None,
                                };

                                if let Some(notification) = notification {
                                    let service = service.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = service.notify(notification).await {
                                            warn!("Failed to dispatch notification: {}", e);
                                        }
                                    });
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Download event listener lagged {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Download event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Listen for pipeline events.
    fn listen_for_pipeline_events(self: &Arc<Self>, mut rx: broadcast::Receiver<PipelineEvent>) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();

        tokio::spawn(async move {
            let mut job_to_streamer: HashMap<String, String> = HashMap::new();
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Pipeline event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                let notification = match event {
                                    PipelineEvent::JobEnqueued { job_id, streamer_id, .. } => {
                                        job_to_streamer.insert(job_id, streamer_id);
                                        None
                                    }
                                    PipelineEvent::JobStarted { job_id, job_type } => {
                                        let streamer_id = job_to_streamer
                                            .get(&job_id)
                                            .cloned()
                                            .unwrap_or_default();
                                        Some(NotificationEvent::PipelineStarted {
                                            job_id,
                                            job_type,
                                            streamer_id,
                                            timestamp: Utc::now(),
                                        })
                                    }
                                    PipelineEvent::JobCompleted {
                                        job_id,
                                        job_type,
                                        duration_secs,
                                    } => {
                                        job_to_streamer.remove(&job_id);
                                        Some(NotificationEvent::PipelineCompleted {
                                            job_id,
                                            job_type,
                                            output_path: None,
                                            duration_secs,
                                            timestamp: Utc::now(),
                                        })
                                    }
                                    PipelineEvent::JobFailed {
                                        job_id,
                                        job_type,
                                        error,
                                    } => {
                                        job_to_streamer.remove(&job_id);
                                        Some(NotificationEvent::PipelineFailed {
                                            job_id,
                                            job_type,
                                            error_message: error,
                                            timestamp: Utc::now(),
                                        })
                                    }
                                    PipelineEvent::QueueWarning { depth } => {
                                        Some(NotificationEvent::PipelineQueueWarning {
                                            queue_depth: depth,
                                            threshold: 100, // TODO: Get from config
                                            timestamp: Utc::now(),
                                        })
                                    }
                                    PipelineEvent::QueueCritical { depth } => {
                                        Some(NotificationEvent::PipelineQueueCritical {
                                            queue_depth: depth,
                                            threshold: 200, // TODO: Get from config
                                            timestamp: Utc::now(),
                                        })
                                    }
                                };

                                if let Some(notification) = notification {
                                    let service = service.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = service.notify(notification).await {
                                            warn!("Failed to dispatch notification: {}", e);
                                        }
                                    });
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Pipeline event listener lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Pipeline event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Stop the notification service.
    pub async fn stop(&self) {
        info!("Stopping notification service");
        self.cancellation_token.cancel();

        // Close web push channel and wait briefly for the worker to flush queued events.
        let tx = self.web_push_tx.write().take();
        if let Some(tx) = tx {
            drop(tx);
        }
        let handle = self.web_push_worker_handle.write().take();
        if let Some(handle) = handle {
            let mut handle = handle;
            if tokio::time::timeout(Duration::from_secs(10), &mut handle)
                .await
                .is_err()
            {
                warn!("Web push worker did not shut down in time; aborting");
                handle.abort();
            }
        }

        // Process any remaining notifications
        let pending_ids: Vec<u64> = self.pending_queue.iter().map(|e| *e.key()).collect();
        for id in pending_ids {
            self.process_notification(id).await;
        }

        info!("Notification service stopped");
    }
}

fn parse_notification_priority(value: &str) -> Option<NotificationPriority> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some(NotificationPriority::Low),
        "normal" => Some(NotificationPriority::Normal),
        "high" => Some(NotificationPriority::High),
        "critical" => Some(NotificationPriority::Critical),
        _ => None,
    }
}

fn normalize_channel_key_part(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "_".to_string();
    }

    trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the notification service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationStats {
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
    use crate::notification::channels::DiscordConfig;

    #[test]
    fn test_notification_service_config_default() {
        let config = NotificationServiceConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_queue_size, 1000);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.circuit_breaker_threshold, 10);
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
        service
            .channels_by_key
            .insert(ok_channel.key.clone(), ok_channel.clone());
        service.channels.write().push(ok_channel);
        service
            .circuit_breakers
            .insert("ok".to_string(), CircuitBreakerState::new(1));

        let flaky_channel = Arc::new(RuntimeChannel {
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
        service
            .channels_by_key
            .insert(flaky_channel.key.clone(), flaky_channel.clone());
        service.channels.write().push(flaky_channel);
        service
            .circuit_breakers
            .insert("flaky".to_string(), CircuitBreakerState::new(1));

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

    struct MockNotificationRepo {
        channels: tokio::sync::Mutex<Vec<NotificationChannelDbModel>>,
        subscriptions: tokio::sync::Mutex<HashMap<String, Vec<String>>>,
        subscribe_calls: tokio::sync::Mutex<Vec<(String, String)>>,
        unsubscribe_calls: tokio::sync::Mutex<Vec<(String, String)>>,
        dead_letters: tokio::sync::Mutex<Vec<NotificationDeadLetterDbModel>>,
    }

    impl MockNotificationRepo {
        fn new() -> Self {
            Self {
                channels: tokio::sync::Mutex::new(Vec::new()),
                subscriptions: tokio::sync::Mutex::new(HashMap::new()),
                subscribe_calls: tokio::sync::Mutex::new(Vec::new()),
                unsubscribe_calls: tokio::sync::Mutex::new(Vec::new()),
                dead_letters: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl NotificationRepository for MockNotificationRepo {
        async fn get_channel(&self, _id: &str) -> Result<NotificationChannelDbModel> {
            unimplemented!()
        }

        async fn list_channels(&self) -> Result<Vec<NotificationChannelDbModel>> {
            Ok(self.channels.lock().await.clone())
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

        async fn cleanup_old_dead_letters(&self, _retention_days: i32) -> Result<i32> {
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
        service
            .channels_by_key
            .insert(channel_1.key.clone(), channel_1.clone());
        service.channels.write().push(channel_1);
        service
            .circuit_breakers
            .insert("channel-1".to_string(), CircuitBreakerState::new(1));

        let channel_2 = Arc::new(RuntimeChannel {
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
        service
            .channels_by_key
            .insert(channel_2.key.clone(), channel_2.clone());
        service.channels.write().push(channel_2);
        service
            .circuit_breakers
            .insert("channel-2".to_string(), CircuitBreakerState::new(1));

        service
            .subscriptions_by_event
            .write()
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
        service
            .channels_by_key
            .insert(fail_channel.key.clone(), fail_channel.clone());
        service.channels.write().push(fail_channel);
        service
            .circuit_breakers
            .insert("channel-1".to_string(), CircuitBreakerState::new(1));

        service
            .subscriptions_by_event
            .write()
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
            let subs = service.subscriptions_by_event.read();
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
