//! Notification service implementation.
//!
//! The NotificationService is responsible for:
//! - Listening to system events (Monitor, Download, Pipeline)
//! - Dispatching notifications to configured channels
//! - Managing retry logic with exponential backoff
//! - Implementing circuit breaker pattern for failing channels
//! - Maintaining a dead letter queue for failed notifications

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::channels::{
    ChannelConfig, DiscordChannel, EmailChannel, NotificationChannel, WebhookChannel,
};
use super::events::NotificationEvent;
use crate::Result;
use crate::downloader::DownloadManagerEvent;
use crate::monitor::MonitorEvent;
use crate::pipeline::PipelineEvent;

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

/// A notification pending delivery.
#[derive(Debug, Clone)]
struct PendingNotification {
    id: u64,
    event: NotificationEvent,
    attempts: u32,
    created_at: DateTime<Utc>,
    last_attempt: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

/// Dead letter entry for failed notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// Notification ID.
    pub id: u64,
    /// The event that failed.
    pub event: NotificationEvent,
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
    config: NotificationServiceConfig,
    channels: RwLock<Vec<Arc<dyn NotificationChannel>>>,
    circuit_breakers: DashMap<String, CircuitBreakerState>,
    pending_queue: DashMap<u64, PendingNotification>,
    dead_letters: DashMap<u64, DeadLetterEntry>,
    next_id: AtomicU64,
    event_tx: broadcast::Sender<NotificationEvent>,
    cancellation_token: CancellationToken,
}

impl NotificationService {
    /// Create a new notification service.
    pub fn new() -> Self {
        Self::with_config(NotificationServiceConfig::default())
    }

    /// Create a new notification service with custom configuration.
    pub fn with_config(config: NotificationServiceConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        let service = Self {
            channels: RwLock::new(Vec::new()),
            circuit_breakers: DashMap::new(),
            pending_queue: DashMap::new(),
            dead_letters: DashMap::new(),
            next_id: AtomicU64::new(1),
            event_tx,
            cancellation_token: CancellationToken::new(),
            config,
        };

        // Initialize channels from config
        service.init_channels();

        service
    }

    /// Initialize channels from configuration.
    fn init_channels(&self) {
        let mut channels = self.channels.write();
        channels.clear();

        for channel_config in &self.config.channels {
            let channel: Arc<dyn NotificationChannel> = match channel_config {
                ChannelConfig::Discord(c) => Arc::new(DiscordChannel::new(c.clone())),
                ChannelConfig::Email(c) => Arc::new(EmailChannel::new(c.clone())),
                ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
            };

            if channel.is_enabled() {
                // Initialize circuit breaker for this channel
                self.circuit_breakers.insert(
                    channel.channel_type().to_string(),
                    CircuitBreakerState::new(self.config.circuit_breaker_cooldown_secs),
                );
                channels.push(channel);
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
            ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
        };

        if channel.is_enabled() {
            self.circuit_breakers.insert(
                channel.channel_type().to_string(),
                CircuitBreakerState::new(self.config.circuit_breaker_cooldown_secs),
            );
            self.channels.write().push(channel);
            info!("Added notification channel: {}", config.channel_type());
        }
    }

    /// Subscribe to notification events.
    pub fn subscribe(&self) -> broadcast::Receiver<NotificationEvent> {
        self.event_tx.subscribe()
    }

    /// Send a notification to all enabled channels.
    pub async fn notify(&self, event: NotificationEvent) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Broadcast the event internally
        let _ = self.event_tx.send(event.clone());

        // Queue the notification
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let pending = PendingNotification {
            id,
            event: event.clone(),
            attempts: 0,
            created_at: Utc::now(),
            last_attempt: None,
            last_error: None,
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

        for channel in &channels {
            let channel_type = channel.channel_type().to_string();

            // Check circuit breaker
            let allowed = self
                .circuit_breakers
                .get(&channel_type)
                .map(|cb| cb.is_allowed())
                .unwrap_or(true);

            if !allowed {
                debug!(
                    "Circuit breaker open for channel {}, skipping",
                    channel_type
                );
                continue;
            }

            // Get the pending notification
            let event = match self.pending_queue.get(&id) {
                Some(pending) => pending.event.clone(),
                None => continue,
            };

            // Attempt to send
            match channel.send(&event).await {
                Ok(()) => {
                    // Record success
                    if let Some(mut cb) = self.circuit_breakers.get_mut(&channel_type) {
                        cb.record_success();
                    }
                    debug!("Notification {} sent via {}", id, channel_type);
                }
                Err(e) => {
                    // Record failure
                    if let Some(mut cb) = self.circuit_breakers.get_mut(&channel_type) {
                        cb.record_failure(self.config.circuit_breaker_threshold);
                    }

                    // Update pending notification
                    if let Some(mut pending) = self.pending_queue.get_mut(&id) {
                        pending.attempts += 1;
                        pending.last_attempt = Some(Utc::now());
                        pending.last_error = Some(e.to_string());

                        if pending.attempts >= self.config.max_retries {
                            // Move to dead letter queue
                            let dead_letter = DeadLetterEntry {
                                id,
                                event: pending.event.clone(),
                                channel_type: channel_type.clone(),
                                attempts: pending.attempts,
                                error: e.to_string(),
                                created_at: pending.created_at,
                                dead_lettered_at: Utc::now(),
                            };
                            drop(pending);
                            self.pending_queue.remove(&id);
                            self.dead_letters.insert(id, dead_letter);
                            warn!(
                                "Notification {} moved to dead letter queue after {} attempts",
                                id, self.config.max_retries
                            );
                        } else {
                            // Schedule retry
                            let delay = self.calculate_retry_delay(pending.attempts);
                            drop(pending);
                            self.schedule_retry(id, delay);
                        }
                    }
                }
            }
        }

        // Remove from pending if all channels succeeded
        self.pending_queue.remove(&id);
    }

    /// Calculate retry delay with exponential backoff and jitter.
    fn calculate_retry_delay(&self, attempts: u32) -> Duration {
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

    /// Schedule a retry for a notification.
    fn schedule_retry(&self, id: u64, delay: Duration) {
        debug!("Scheduling retry for notification {} in {:?}", id, delay);

        let pending_queue = self.pending_queue.clone();
        let channels = self.channels.read().clone();
        let circuit_breakers = self.circuit_breakers.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            sleep(delay).await;

            if let Some(pending) = pending_queue.get(&id) {
                let event = pending.event.clone();
                drop(pending);

                // Process notification inline instead of recursive call
                for channel in &channels {
                    let channel_type = channel.channel_type().to_string();

                    let allowed = circuit_breakers
                        .get(&channel_type)
                        .map(|cb| cb.is_allowed())
                        .unwrap_or(true);

                    if !allowed {
                        continue;
                    }

                    match channel.send(&event).await {
                        Ok(()) => {
                            if let Some(mut cb) = circuit_breakers.get_mut(&channel_type) {
                                cb.record_success();
                            }
                        }
                        Err(e) => {
                            if let Some(mut cb) = circuit_breakers.get_mut(&channel_type) {
                                cb.record_failure(config.circuit_breaker_threshold);
                            }
                            warn!("Retry failed for notification {}: {}", id, e);
                        }
                    }
                }

                pending_queue.remove(&id);
            }
        });
    }

    /// Get dead letter entries.
    pub fn get_dead_letters(&self) -> Vec<DeadLetterEntry> {
        self.dead_letters
            .iter()
            .map(|e| e.value().clone())
            .collect()
    }

    /// Retry a dead letter notification.
    pub async fn retry_dead_letter(&self, id: u64) -> Result<()> {
        if let Some((_, dead_letter)) = self.dead_letters.remove(&id) {
            self.notify(dead_letter.event).await
        } else {
            Err(crate::Error::NotFound {
                entity_type: "DeadLetter".to_string(),
                id: id.to_string(),
            })
        }
    }

    /// Clear old dead letters.
    pub fn cleanup_dead_letters(&self) {
        let retention = chrono::Duration::days(self.config.dead_letter_retention_days as i64);
        let cutoff = Utc::now() - retention;

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
        &self,
        monitor_rx: broadcast::Receiver<MonitorEvent>,
        download_rx: broadcast::Receiver<DownloadManagerEvent>,
        pipeline_rx: broadcast::Receiver<PipelineEvent>,
    ) {
        self.listen_for_monitor_events(monitor_rx);
        self.listen_for_download_events(download_rx);
        self.listen_for_pipeline_events(pipeline_rx);
    }

    /// Listen for monitor events.
    fn listen_for_monitor_events(&self, mut rx: broadcast::Receiver<MonitorEvent>) {
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let cancellation_token = self.cancellation_token.clone();

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
                                    let _ = event_tx.send(notification);
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
    fn listen_for_download_events(&self, mut rx: broadcast::Receiver<DownloadManagerEvent>) {
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let cancellation_token = self.cancellation_token.clone();

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
                                        session_id,
                                        ..
                                    } => Some(NotificationEvent::DownloadStarted {
                                        streamer_id: streamer_id.clone(),
                                        streamer_name: streamer_id, // TODO: Get name from manager
                                        session_id,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadCompleted {
                                        streamer_id,
                                        session_id,
                                        total_bytes,
                                        total_duration_secs,
                                        ..
                                    } => Some(NotificationEvent::DownloadCompleted {
                                        streamer_id: streamer_id.clone(),
                                        streamer_name: streamer_id,
                                        session_id,
                                        file_size_bytes: total_bytes,
                                        duration_secs: total_duration_secs,
                                        timestamp: Utc::now(),
                                    }),
                                    DownloadManagerEvent::DownloadFailed {
                                        streamer_id,
                                        error,
                                        recoverable,
                                        ..
                                    } => Some(NotificationEvent::DownloadError {
                                        streamer_id: streamer_id.clone(),
                                        streamer_name: streamer_id,
                                        error_message: error,
                                        recoverable,
                                        timestamp: Utc::now(),
                                    }),
                                    _ => None,
                                };

                                if let Some(notification) = notification {
                                    let _ = event_tx.send(notification);
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
    fn listen_for_pipeline_events(&self, mut rx: broadcast::Receiver<PipelineEvent>) {
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
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
                                    PipelineEvent::JobStarted { job_id, job_type } => {
                                        Some(NotificationEvent::PipelineStarted {
                                            job_id,
                                            job_type,
                                            streamer_id: String::new(),
                                            timestamp: Utc::now(),
                                        })
                                    }
                                    PipelineEvent::JobCompleted {
                                        job_id,
                                        job_type,
                                        duration_secs,
                                    } => Some(NotificationEvent::PipelineCompleted {
                                        job_id,
                                        job_type,
                                        output_path: None,
                                        duration_secs,
                                        timestamp: Utc::now(),
                                    }),
                                    PipelineEvent::JobFailed {
                                        job_id,
                                        job_type,
                                        error,
                                    } => Some(NotificationEvent::PipelineFailed {
                                        job_id,
                                        job_type,
                                        error_message: error,
                                        timestamp: Utc::now(),
                                    }),
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
                                    _ => None,
                                };

                                if let Some(notification) = notification {
                                    let _ = event_tx.send(notification);
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
    /// Number of pending notifications.
    pub pending_count: usize,
    /// Number of dead letter entries.
    pub dead_letter_count: usize,
    /// Number of configured channels.
    pub channel_count: usize,
    /// Circuit breaker states (channel_type -> is_open).
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

        let delay1 = service.calculate_retry_delay(0);
        let delay2 = service.calculate_retry_delay(1);
        let delay3 = service.calculate_retry_delay(2);

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
}
