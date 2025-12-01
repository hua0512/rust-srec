//! Generic webhook notification channel.

use async_trait::async_trait;
use reqwest::{Client, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, warn};

use super::NotificationChannel;
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

/// Webhook channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// Webhook URL.
    pub url: String,
    /// HTTP method (default: POST).
    #[serde(default = "default_method")]
    pub method: String,
    /// Custom headers.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Authentication type.
    pub auth: Option<WebhookAuth>,
    /// Minimum priority level to send (default: Normal).
    #[serde(default)]
    pub min_priority: NotificationPriority,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_method() -> String {
    "POST".to_string()
}

fn default_timeout() -> u64 {
    30
}

/// Webhook authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    /// Bearer token authentication.
    Bearer { token: String },
    /// Basic authentication.
    Basic { username: String, password: String },
    /// Custom header authentication.
    Header { name: String, value: String },
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            method: "POST".to_string(),
            headers: Vec::new(),
            auth: None,
            min_priority: NotificationPriority::Normal,
            timeout_secs: 30,
        }
    }
}

/// Generic webhook notification channel.
pub struct WebhookChannel {
    config: WebhookConfig,
    client: Client,
}

impl WebhookChannel {
    /// Create a new Webhook channel.
    pub fn new(config: WebhookConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();

        Self { config, client }
    }

    /// Build the request headers.
    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();

        // Add custom headers
        for (name, value) in &self.config.headers {
            if let (Ok(name), Ok(value)) = (
                name.parse::<reqwest::header::HeaderName>(),
                value.parse::<reqwest::header::HeaderValue>(),
            ) {
                headers.insert(name, value);
            }
        }

        // Add auth header
        if let Some(auth) = &self.config.auth {
            match auth {
                WebhookAuth::Bearer { token } => {
                    if let Ok(value) = format!("Bearer {}", token).parse() {
                        headers.insert(reqwest::header::AUTHORIZATION, value);
                    }
                }
                WebhookAuth::Header { name, value } => {
                    if let (Ok(name), Ok(value)) = (
                        name.parse::<reqwest::header::HeaderName>(),
                        value.parse::<reqwest::header::HeaderValue>(),
                    ) {
                        headers.insert(name, value);
                    }
                }
                WebhookAuth::Basic { .. } => {
                    // Basic auth is handled separately in the request builder
                }
            }
        }

        headers
    }

    /// Build the JSON payload.
    fn build_payload(&self, event: &NotificationEvent) -> serde_json::Value {
        json!({
            "event_type": event.event_type(),
            "priority": event.priority().to_string(),
            "title": event.title(),
            "description": event.description(),
            "timestamp": event.timestamp().to_rfc3339(),
            "streamer_id": event.streamer_id(),
            "data": event
        })
    }
}

#[async_trait]
impl NotificationChannel for WebhookChannel {
    fn channel_type(&self) -> &'static str {
        "webhook"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled && !self.config.url.is_empty()
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Check priority filter
        if event.priority() < self.config.min_priority {
            debug!(
                "Skipping webhook notification for {} (priority {} < {})",
                event.event_type(),
                event.priority(),
                self.config.min_priority
            );
            return Ok(());
        }

        let payload = self.build_payload(event);
        let headers = self.build_headers();

        let mut request = match self.config.method.to_uppercase().as_str() {
            "POST" => self.client.post(&self.config.url),
            "PUT" => self.client.put(&self.config.url),
            _ => self.client.post(&self.config.url),
        };

        request = request.headers(headers).json(&payload);

        // Add basic auth if configured
        if let Some(WebhookAuth::Basic { username, password }) = &self.config.auth {
            request = request.basic_auth(username, Some(password));
        }

        let response = request
            .send()
            .await
            .map_err(|e| crate::Error::Other(format!("Webhook request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!("Webhook failed: {} - {}", status, body);
            return Err(crate::Error::Other(format!(
                "Webhook failed: {} - {}",
                status, body
            )));
        }

        debug!("Webhook notification sent: {}", event.event_type());
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
    fn test_webhook_config_default() {
        let config = WebhookConfig::default();
        assert!(!config.enabled);
        assert!(config.url.is_empty());
        assert_eq!(config.method, "POST");
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_webhook_channel_disabled() {
        let config = WebhookConfig::default();
        let channel = WebhookChannel::new(config);
        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_build_payload() {
        let config = WebhookConfig::default();
        let channel = WebhookChannel::new(config);

        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            timestamp: chrono::Utc::now(),
        };

        let payload = channel.build_payload(&event);
        assert_eq!(payload["event_type"], "stream_online");
        assert_eq!(payload["streamer_id"], "123");
    }

    #[test]
    fn test_build_headers_with_bearer() {
        let config = WebhookConfig {
            enabled: true,
            url: "https://example.com/webhook".to_string(),
            auth: Some(WebhookAuth::Bearer {
                token: "test-token".to_string(),
            }),
            ..Default::default()
        };
        let channel = WebhookChannel::new(config);
        let headers = channel.build_headers();

        assert!(headers.contains_key(reqwest::header::AUTHORIZATION));
    }
}
