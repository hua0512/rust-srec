//! Gotify notification channel.
//!
//! Sends messages via the Gotify REST API (`POST /message?token=<app_token>`).

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, warn};

use super::NotificationChannel;
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

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
            .field("server_url", &self.server_url)
            .field("app_token", &"[REDACTED]")
            .field("min_priority", &self.min_priority)
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
            timeout_secs: default_timeout(),
        }
    }
}

/// Gotify notification channel.
pub struct GotifyChannel {
    config: GotifyConfig,
    client: Client,
    /// Pre-computed message endpoint (without token query param).
    message_url: String,
}

impl GotifyChannel {
    /// Create a new Gotify channel.
    pub fn new(config: GotifyConfig) -> Self {
        crate::utils::http_client::install_rustls_provider();
        let message_url = format!("{}/message", config.server_url.trim_end_matches('/'));
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        Self {
            config,
            client,
            message_url,
        }
    }

    /// Build the Gotify message payload.
    fn build_payload(&self, event: &NotificationEvent) -> serde_json::Value {
        json!({
            "title": event.title(),
            "message": event.description(),
            "priority": event.priority().as_int(),
        })
    }
}

impl NotificationChannel for GotifyChannel {
    fn channel_type(&self) -> &'static str {
        "gotify"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
            && !self.config.server_url.is_empty()
            && !self.config.app_token.is_empty()
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Check priority filter
        if event.priority() < self.config.min_priority {
            debug!(
                "Skipping Gotify notification for {} (priority {} < {})",
                event.event_type(),
                event.priority(),
                self.config.min_priority
            );
            return Ok(());
        }

        let payload = self.build_payload(event);

        let response = self
            .client
            .post(&self.message_url)
            .query(&[("token", &self.config.app_token)])
            .json(&payload)
            .send()
            .await
            .map_err(|e| crate::Error::Other(format!("Gotify request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!("Gotify request failed: {} - {}", status, body);
            return Err(crate::Error::Other(format!(
                "Gotify request failed: {} - {}",
                status, body
            )));
        }

        debug!("Gotify notification sent: {}", event.event_type());
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
    fn test_gotify_config_default() {
        let config = GotifyConfig::default();
        assert!(!config.enabled);
        assert!(config.server_url.is_empty());
        assert!(config.app_token.is_empty());
        assert_eq!(config.min_priority, NotificationPriority::Normal);
        assert_eq!(config.timeout_secs, 30);
    }

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
}
