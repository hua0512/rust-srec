//! Gotify notification channel.
//!
//! Sends messages via the Gotify REST API (`POST /message?token=<app_token>`).

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

use super::http::{HttpDelivery, RetryPolicy};
use super::{NotificationChannel, send_direct};
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority, RenderedEvent};

/// Gotify channel configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct GotifyConfig {
    /// Stable channel instance identifier (recommended).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional display name for this channel instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// Gotify server base URL (e.g. `https://gotify.example.com`).
    pub server_url: String,
    /// Gotify application token.
    pub app_token: String,
    /// Minimum priority level to send (default: Normal).
    #[serde(default)]
    pub min_priority: NotificationPriority,
    /// Language for the rendered title and body; `None` follows the process-wide locale.
    /// See [`NotificationEvent::title_for`] for locale fallback.
    #[serde(default)]
    pub locale: Option<String>,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl std::fmt::Debug for GotifyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GotifyConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("server_url", &"[REDACTED]")
            .field("app_token", &"[REDACTED]")
            .field("min_priority", &self.min_priority)
            .field("locale", &self.locale)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

fn default_timeout() -> u64 {
    30
}

impl Default for GotifyConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            enabled: false,
            server_url: String::new(),
            app_token: String::new(),
            min_priority: NotificationPriority::Normal,
            locale: None,
            timeout_secs: default_timeout(),
        }
    }
}

/// Gotify notification channel.
pub struct GotifyChannel {
    config: GotifyConfig,
    http: HttpDelivery,
    /// Pre-computed message endpoint (without token query param).
    message_url: String,
}

impl GotifyChannel {
    /// Create a new Gotify channel.
    pub fn new(config: GotifyConfig) -> Self {
        let message_url = format!("{}/message", config.server_url.trim_end_matches('/'));
        let http = HttpDelivery::new(
            "gotify",
            std::time::Duration::from_secs(config.timeout_secs),
        );
        Self {
            config,
            http,
            message_url,
        }
    }

    /// Build the Gotify message payload.
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
        json!({
            "title": rendered.title,
            "message": rendered.description,
            "priority": event.priority().as_int(),
        })
    }
}

#[async_trait]
impl NotificationChannel for GotifyChannel {
    fn channel_type(&self) -> &'static str {
        "gotify"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
            && !self.config.server_url.is_empty()
            && !self.config.app_token.is_empty()
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

        let request = self
            .http
            .request(Method::POST, &self.message_url)?
            .query(&[("token", &self.config.app_token)])
            .json(&payload);
        self.http.send(request, RetryPolicy::None).await?;

        debug!("Gotify notification sent: {}", event.event_type());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gotify_channel_disabled() {
        let config = GotifyConfig::default();
        let channel = GotifyChannel::new(config);
        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_gotify_channel_enabled() {
        let config = GotifyConfig {
            enabled: true,
            server_url: "https://gotify.example.com".to_string(),
            app_token: "test-token".to_string(),
            ..Default::default()
        };
        let channel = GotifyChannel::new(config);
        assert!(channel.is_enabled());
    }

    #[test]
    fn test_build_payload() {
        let config = GotifyConfig::default();
        let channel = GotifyChannel::new(config);

        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            timestamp: chrono::Utc::now(),
        };

        let payload = channel.build_payload(&event);
        assert!(payload["title"].as_str().is_some());
        assert!(payload["message"].as_str().is_some());
        assert_eq!(payload["priority"], 5); // Normal = 5
    }

    #[test]
    fn supplied_rendering_matches_direct_payloads_in_both_locales() {
        let event = NotificationEvent::SystemStartup {
            version: "<test>&😀".into(),
            timestamp: chrono::Utc::now(),
        };
        for locale in ["en", "zh-CN"] {
            let channel = GotifyChannel::new(GotifyConfig {
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
            assert_eq!(payload["title"], supplied.title);
            assert_eq!(payload["message"], supplied.description);
        }
    }
}
