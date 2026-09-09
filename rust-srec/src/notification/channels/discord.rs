//! Discord webhook notification channel.
//!
//! Implements Discord's recommended rate limit handling:
//! - No hardcoded rate limits
//! - Parses response headers (X-RateLimit-*)
//! - Retries on 429 responses respecting Retry-After header

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

use super::http::{DEFAULT_TIMEOUT, HttpDelivery, RetryPolicy};
use super::{NotificationChannel, send_direct};
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority, RenderedEvent};

/// Discord channel configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Stable channel instance identifier (recommended).
    ///
    /// When provided, this is used to derive the runtime channel key so reordering the config
    /// does not reset circuit breaker history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional display name for this channel instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// Language for the rendered title and body; `None` follows the process-wide locale.
    /// See [`NotificationEvent::title_for`] for locale fallback.
    #[serde(default)]
    pub locale: Option<String>,
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("webhook_url", &"[REDACTED]")
            .field("username", &self.username)
            .field(
                "avatar_url",
                &self.avatar_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("min_priority", &self.min_priority)
            .field("locale", &self.locale)
            .finish()
    }
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            enabled: false,
            webhook_url: String::new(),
            username: Some("rust-srec".to_string()),
            avatar_url: None,
            min_priority: NotificationPriority::Normal,
            locale: None,
        }
    }
}

/// Discord notification channel.
pub struct DiscordChannel {
    config: DiscordConfig,
    http: HttpDelivery,
}

impl DiscordChannel {
    /// Create a new Discord channel.
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            http: HttpDelivery::new("discord", DEFAULT_TIMEOUT),
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

    /// Build the webhook payload for an event.
    #[cfg(test)]
    fn build_payload(&self, event: &NotificationEvent) -> serde_json::Value {
        let locale = self
            .config
            .locale
            .clone()
            .unwrap_or_else(crate::i18n::current_locale);
        self.build_rendered_payload(event, &RenderedEvent::new(event, &locale))
    }

    fn build_rendered_payload(
        &self,
        event: &NotificationEvent,
        rendered: &RenderedEvent,
    ) -> serde_json::Value {
        let embed = json!({
            "title": rendered.title,
            "description": rendered.description,
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

        payload
    }

    async fn send_with_retry(&self, payload: &serde_json::Value) -> Result<()> {
        let request = self
            .http
            .request(Method::POST, &self.config.webhook_url)?
            .json(payload);
        self.http.send(request, RetryPolicy::Discord).await
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

    fn locale(&self) -> Option<&str> {
        self.config.locale.as_deref()
    }

    fn min_priority(&self) -> NotificationPriority {
        self.config.min_priority
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        send_direct(self, event).await
    }

    async fn send_rendered(
        &self,
        event: &NotificationEvent,
        rendered: &RenderedEvent,
    ) -> Result<()> {
        if !self.accepts(event) {
            return Ok(());
        }

        let payload = self.build_rendered_payload(event, rendered);
        self.send_with_retry(&payload).await?;

        debug!("Discord notification sent: {}", event.event_type());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_build_payload() {
        let config = DiscordConfig::default();
        let channel = DiscordChannel::new(config);

        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            timestamp: chrono::Utc::now(),
        };

        let payload = channel.build_payload(&event);

        // Verify embed structure
        assert!(payload["embeds"].is_array());
        let embed = &payload["embeds"][0];
        assert!(embed["title"].as_str().unwrap().contains("TestStreamer"));
        assert!(
            embed["description"]
                .as_str()
                .unwrap()
                .contains("Playing Games")
        );
        assert_eq!(
            embed["color"],
            DiscordChannel::get_color(NotificationPriority::Normal) as i64
        );
    }

    #[test]
    fn test_build_payload_with_custom_username() {
        let config = DiscordConfig {
            enabled: true,
            webhook_url: "https://example.com".to_string(),
            username: Some("CustomBot".to_string()),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            ..Default::default()
        };
        let channel = DiscordChannel::new(config);

        let event = NotificationEvent::SystemStartup {
            version: "1.0.0".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let payload = channel.build_payload(&event);

        assert_eq!(payload["username"], "CustomBot");
        assert_eq!(payload["avatar_url"], "https://example.com/avatar.png");
    }

    #[test]
    fn supplied_rendering_matches_direct_payloads_in_both_locales() {
        let event = NotificationEvent::SystemStartup {
            version: "<test>&😀".into(),
            timestamp: chrono::Utc::now(),
        };
        for locale in ["en", "zh-CN"] {
            let channel = DiscordChannel::new(DiscordConfig {
                locale: Some(locale.into()),
                ..Default::default()
            });
            let rendered = RenderedEvent::new(&event, locale);
            assert_eq!(
                channel.build_payload(&event),
                channel.build_rendered_payload(&event, &rendered)
            );
            let supplied = RenderedEvent {
                title: "unique title 😀".into(),
                description: "unique body <>&".into(),
            };
            let payload = channel.build_rendered_payload(&event, &supplied);
            assert_eq!(payload["embeds"][0]["title"], supplied.title);
            assert_eq!(payload["embeds"][0]["description"], supplied.description);
        }
    }
}
