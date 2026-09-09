//! Notification channels.
//!
//! This module provides different channels for delivering notifications:
//! - Discord webhooks
//! - Email (SMTP)
//! - Telegram Bot API
//! - Generic webhooks (HTTP POST)

mod discord;
mod email;
mod gotify;
mod http;
mod telegram;
mod webhook;

#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod redaction_tests;

pub use discord::{DiscordChannel, DiscordConfig};
pub use email::{EmailChannel, EmailConfig};
pub use gotify::{GotifyChannel, GotifyConfig};
pub use telegram::{TelegramChannel, TelegramConfig};
pub use webhook::{WebhookAuth, WebhookChannel, WebhookConfig};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::events::{NotificationEvent, NotificationPriority, RenderedEvent};
use crate::Result;

/// Trait for notification channels.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Get the channel type name.
    fn channel_type(&self) -> &'static str;

    /// Check if the channel is enabled.
    fn is_enabled(&self) -> bool;

    /// Send a notification through this channel.
    async fn send(&self, event: &NotificationEvent) -> Result<()>;

    /// Test the channel configuration.
    async fn test(&self) -> Result<()> {
        self.send(&NotificationEvent::SystemStartup {
            version: "test".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
    }

    /// The channel's configured locale; unset follows the delivery pass snapshot.
    fn locale(&self) -> Option<&str> {
        None
    }

    /// Default preserves external implementations that own their filtering in send.
    fn min_priority(&self) -> NotificationPriority {
        NotificationPriority::Low
    }

    /// Shared built-in filtering, including transport-specific readiness.
    fn accepts(&self, event: &NotificationEvent) -> bool {
        if !self.is_enabled() {
            return false;
        }
        if event.priority() < self.min_priority() {
            tracing::debug!(channel = self.channel_type(), event_type = event.event_type(),
                priority = %event.priority(), min_priority = %self.min_priority(),
                "Skipping notification below channel priority");
            return false;
        }
        true
    }

    /// Built-ins consume shared text; external channels keep their existing send
    /// implementation without having to adopt this optional rendering interface.
    async fn send_rendered(
        &self,
        event: &NotificationEvent,
        _rendered: &RenderedEvent,
    ) -> Result<()> {
        self.send(event).await
    }
}

/// Channel configuration wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    /// Discord webhook channel.
    Discord(DiscordConfig),
    /// Email channel.
    Email(EmailConfig),
    /// Gotify push notification channel.
    Gotify(GotifyConfig),
    /// Telegram Bot API channel.
    Telegram(TelegramConfig),
    /// Generic webhook channel.
    Webhook(WebhookConfig),
}

impl ChannelConfig {
    /// Get the channel type name.
    pub fn channel_type(&self) -> &'static str {
        match self {
            Self::Discord(_) => "discord",
            Self::Email(_) => "email",
            Self::Gotify(_) => "gotify",
            Self::Telegram(_) => "telegram",
            Self::Webhook(_) => "webhook",
        }
    }

    /// Check if the channel is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Discord(c) => c.enabled,
            Self::Email(c) => c.enabled,
            Self::Gotify(c) => c.enabled,
            Self::Telegram(c) => c.enabled,
            Self::Webhook(c) => c.enabled,
        }
    }

    /// Optional stable channel instance identifier.
    pub fn instance_id(&self) -> Option<&str> {
        match self {
            Self::Discord(c) => c.id.as_deref(),
            Self::Email(c) => c.id.as_deref(),
            Self::Gotify(c) => c.id.as_deref(),
            Self::Telegram(c) => c.id.as_deref(),
            Self::Webhook(c) => c.id.as_deref(),
        }
    }

    /// Optional human-friendly display name.
    pub fn display_name(&self) -> Option<&str> {
        match self {
            Self::Discord(c) => c.name.as_deref(),
            Self::Email(c) => c.name.as_deref(),
            Self::Gotify(c) => c.name.as_deref(),
            Self::Telegram(c) => c.name.as_deref(),
            Self::Webhook(c) => c.name.as_deref(),
        }
    }
}

/// Direct sends use the same filtering and payload path as service deliveries.
async fn send_direct(channel: &dyn NotificationChannel, event: &NotificationEvent) -> Result<()> {
    if !channel.accepts(event) {
        return Ok(());
    }
    let locale = channel
        .locale()
        .map(str::to_owned)
        .unwrap_or_else(crate::i18n::current_locale);
    channel
        .send_rendered(event, &RenderedEvent::new(event, &locale))
        .await
}
