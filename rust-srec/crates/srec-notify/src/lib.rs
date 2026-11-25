//! Notification system module.
//!
//! Provides notification delivery for system events through multiple channels
//! (Discord, Email, Webhook) with retry logic and circuit breaker pattern.
//!
//! # Features
//!
//! - Multiple notification channels (Discord, Email, Webhook)
//! - Priority-based filtering
//! - Retry with exponential backoff
//! - Circuit breaker pattern for failing channels
//! - Dead letter queue for failed notifications
//! - Event listeners for Monitor, Download, and Pipeline events
//!
//! # Example
//!
//! ```ignore
//! use srec_notify::{NotificationService, NotificationServiceConfig};
//! use srec_notify::channels::{ChannelConfig, DiscordConfig};
//!
//! let config = NotificationServiceConfig {
//!     enabled: true,
//!     channels: vec![
//!         ChannelConfig::Discord(DiscordConfig {
//!             enabled: true,
//!             webhook_url: "https://discord.com/api/webhooks/...".to_string(),
//!             ..Default::default()
//!         }),
//!     ],
//!     ..Default::default()
//! };
//!
//! let service = NotificationService::with_config(config);
//! ```

// Embed locale YAML files at compile time. Must be invoked at the crate root
// because the `t!` macro generates code that resolves `_rust_i18n_t` via
// `crate::_rust_i18n_t`. See `crate::i18n` for the wrapper module that
// exposes locale-management helpers and internal `t_str_in!`.
rust_i18n::i18n!("locales", fallback = "en");

pub mod channels;
pub mod error;
pub mod events;
pub mod i18n;
pub mod outbox_drainer;
pub mod service;
pub mod web_push;

pub use channels::{ChannelConfig, DiscordConfig, EmailConfig, NotificationChannel, WebhookConfig};
pub use error::{NotifyError, NotifyResult};
pub use events::{BigoNotificationProblem, NotificationEvent, NotificationPriority};
pub use outbox_drainer::{
    LegacyOutboxEvent, NotificationOutboxDrainer, OutboxDrainReport, OutboxRecord,
    outbox_event_to_notification,
};
pub use service::{
    DeadLetterEntry, DurableDeliveryError, NotificationChannelInstance, NotificationChannelSource,
    NotificationService, NotificationServiceConfig, NotificationStats,
};
