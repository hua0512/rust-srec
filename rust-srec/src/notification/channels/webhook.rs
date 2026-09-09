//! Generic webhook notification channel.

use async_trait::async_trait;
use reqwest::{Method, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

use super::http::{HttpDelivery, RetryPolicy};
use super::{NotificationChannel, send_direct};
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority, RenderedEvent};

/// Webhook channel configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
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
    /// Language for the rendered title and body; `None` follows the process-wide locale.
    /// See [`NotificationEvent::title_for`] for locale fallback.
    #[serde(default)]
    pub locale: Option<String>,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<_> = self
            .headers
            .iter()
            .map(|(name, _)| (name, "[REDACTED]"))
            .collect();
        f.debug_struct("WebhookConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("url", &"[REDACTED]")
            .field("method", &self.method)
            .field("headers", &headers)
            .field("auth", &self.auth)
            .field("min_priority", &self.min_priority)
            .field("locale", &self.locale)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

fn default_method() -> String {
    "POST".to_string()
}

fn default_timeout() -> u64 {
    30
}

/// Webhook authentication configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    /// Bearer token authentication.
    Bearer { token: String },
    /// Basic authentication.
    Basic { username: String, password: String },
    /// Custom header authentication.
    Header { name: String, value: String },
}

impl std::fmt::Debug for WebhookAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer { .. } => f
                .debug_struct("Bearer")
                .field("token", &"[REDACTED]")
                .finish(),
            Self::Basic { .. } => f
                .debug_struct("Basic")
                .field("username", &"[REDACTED]")
                .field("password", &"[REDACTED]")
                .finish(),
            Self::Header { name, .. } => f
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"[REDACTED]")
                .finish(),
        }
    }
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            enabled: false,
            url: String::new(),
            method: "POST".to_string(),
            headers: Vec::new(),
            auth: None,
            min_priority: NotificationPriority::Normal,
            locale: None,
            timeout_secs: 30,
        }
    }
}

/// Generic webhook notification channel.
pub struct WebhookChannel {
    config: WebhookConfig,
    http: HttpDelivery,
}

impl WebhookChannel {
    /// Create a new Webhook channel.
    pub fn new(config: WebhookConfig) -> Self {
        let http = HttpDelivery::new(
            "webhook",
            std::time::Duration::from_secs(config.timeout_secs),
        );

        Self { config, http }
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
            "event_type": event.event_type(),
            "priority": event.priority().as_int(),
            "priority_label": event.priority().to_string(),
            "title": rendered.title,
            "description": rendered.description,
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
        let headers = self.build_headers();

        let mut request = match self.config.method.to_uppercase().as_str() {
            "POST" => self.http.request(Method::POST, &self.config.url)?,
            "PUT" => self.http.request(Method::PUT, &self.config.url)?,
            _ => self.http.request(Method::POST, &self.config.url)?,
        };

        request = request.headers(headers).json(&payload);

        // Add basic auth if configured
        if let Some(WebhookAuth::Basic { username, password }) = &self.config.auth {
            request = request.basic_auth(username, Some(password));
        }

        self.http.send(request, RetryPolicy::None).await?;

        debug!("Webhook notification sent: {}", event.event_type());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(payload["priority"], 5); // Normal = 5 (integer)
        assert_eq!(payload["priority_label"], "normal");
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

    #[test]
    fn supplied_rendering_matches_direct_payloads_in_both_locales() {
        let event = NotificationEvent::SystemStartup {
            version: "<test>&😀".into(),
            timestamp: chrono::Utc::now(),
        };
        for locale in ["en", "zh-CN"] {
            let channel = WebhookChannel::new(WebhookConfig {
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
            assert_eq!(payload["description"], supplied.description);
            assert_eq!(payload["data"], serde_json::to_value(&event).unwrap());
        }
    }
}
