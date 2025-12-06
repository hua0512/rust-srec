//! Discord webhook notification channel.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, warn};

use super::NotificationChannel;
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

/// Discord channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// Discord webhook URL.
    pub webhook_url: String,
    /// Optional username for the webhook.
    pub username: Option<String>,
    /// Optional avatar URL for the webhook.
    pub avatar_url: Option<String>,
    /// Minimum priority level to send (default: Normal).
    #[serde(default)]
    pub min_priority: NotificationPriority,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            webhook_url: String::new(),
            username: Some("rust-srec".to_string()),
            avatar_url: None,
            min_priority: NotificationPriority::Normal,
        }
    }
}

/// Discord notification channel.
pub struct DiscordChannel {
    config: DiscordConfig,
    client: Client,
}

impl DiscordChannel {
    /// Create a new Discord channel.
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    /// Get the embed color based on priority.
    fn get_color(priority: NotificationPriority) -> u32 {
        match priority {
            NotificationPriority::Low => 0x808080,      // Gray
            NotificationPriority::Normal => 0x3498db,   // Blue
            NotificationPriority::High => 0xf39c12,     // Orange
            NotificationPriority::Critical => 0xe74c3c, // Red
        }
    }
}

#[async_trait]
impl NotificationChannel for DiscordChannel {
    fn channel_type(&self) -> &'static str {
        "discord"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled && !self.config.webhook_url.is_empty()
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Check priority filter
        if event.priority() < self.config.min_priority {
            debug!(
                "Skipping Discord notification for {} (priority {} < {})",
                event.event_type(),
                event.priority(),
                self.config.min_priority
            );
            return Ok(());
        }

        let embed = json!({
            "title": event.title(),
            "description": event.description(),
            "color": Self::get_color(event.priority()),
            "timestamp": event.timestamp().to_rfc3339(),
            "footer": {
                "text": format!("Priority: {} | Type: {}", event.priority(), event.event_type())
            }
        });

        let mut payload = json!({
            "embeds": [embed]
        });

        if let Some(username) = &self.config.username {
            payload["username"] = json!(username);
        }
        if let Some(avatar_url) = &self.config.avatar_url {
            payload["avatar_url"] = json!(avatar_url);
        }

        let response = self
            .client
            .post(&self.config.webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| crate::Error::Other(format!("Discord request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!("Discord webhook failed: {} - {}", status, body);
            return Err(crate::Error::Other(format!(
                "Discord webhook failed: {} - {}",
                status, body
            )));
        }

        debug!("Discord notification sent: {}", event.event_type());
        Ok(())
    }

    async fn test(&self) -> Result<()> {
        let test_event = NotificationEvent::SystemStartup {
            version: "test".to_string(),
            timestamp: chrono::Utc::now(),
        };
        self.send(&test_event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_config_default() {
        let config = DiscordConfig::default();
        assert!(!config.enabled);
        assert!(config.webhook_url.is_empty());
        assert_eq!(config.min_priority, NotificationPriority::Normal);
    }

    #[test]
    fn test_discord_channel_disabled() {
        let config = DiscordConfig::default();
        let channel = DiscordChannel::new(config);
        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_get_color() {
        assert_eq!(
            DiscordChannel::get_color(NotificationPriority::Low),
            0x808080
        );
        assert_eq!(
            DiscordChannel::get_color(NotificationPriority::Critical),
            0xe74c3c
        );
    }
}
