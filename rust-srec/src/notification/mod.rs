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
//! use rust_srec::notification::{NotificationService, NotificationServiceConfig};
//! use rust_srec::notification::channels::{ChannelConfig, DiscordConfig};
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

pub mod channels;
pub mod events;
pub mod service;

pub use channels::{ChannelConfig, DiscordConfig, EmailConfig, NotificationChannel, WebhookConfig};
pub use events::{NotificationEvent, NotificationPriority};
pub use service::{DeadLetterEntry, NotificationService, NotificationServiceConfig, NotificationStats};
