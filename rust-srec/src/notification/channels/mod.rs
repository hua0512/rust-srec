//! Notification channels.
//!
//! This module provides different channels for delivering notifications:
//! - Discord webhooks
//! - Email (SMTP)
//! - Generic webhooks (HTTP POST)

mod discord;
mod email;
mod webhook;

pub use discord::{DiscordChannel, DiscordConfig};
pub use email::{EmailChannel, EmailConfig};
pub use webhook::{WebhookChannel, WebhookConfig};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::events::NotificationEvent;
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
    async fn test(&self) -> Result<()>;
}

/// Channel configuration wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    /// Discord webhook channel.
    Discord(DiscordConfig),
    /// Email channel.
    Email(EmailConfig),
    /// Generic webhook channel.
    Webhook(WebhookConfig),
}

impl ChannelConfig {
    /// Get the channel type name.
    pub fn channel_type(&self) -> &'static str {
        match self {
            Self::Discord(_) => "discord",
            Self::Email(_) => "email",
            Self::Webhook(_) => "webhook",
        }
    }

    /// Check if the channel is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Discord(c) => c.enabled,
            Self::Email(c) => c.enabled,
            Self::Webhook(c) => c.enabled,
        }
    }
}
