//! Telegram Bot API notification channel.
//!
//! Sends messages via the Telegram Bot API (`POST /bot<token>/sendMessage`).
//! Handles 429 rate limits by respecting the `parameters.retry_after` field
//! returned in the JSON response body.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, warn};

use super::NotificationChannel;
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

/// Maximum number of retries for rate-limited requests.
const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// Telegram `sendMessage` text limit (UTF-8 characters).
const TELEGRAM_MESSAGE_LIMIT: usize = 4096;

/// Telegram channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Stable channel instance identifier (recommended).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional display name for this channel instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// Telegram Bot API token.
    pub bot_token: String,
    /// Target chat ID (user, group, or channel).
    pub chat_id: String,
    /// Parse mode for message formatting (HTML, Markdown, MarkdownV2).
    #[serde(default = "default_parse_mode")]
    pub parse_mode: String,
    /// Minimum priority level to send (default: Normal).
    #[serde(default)]
    pub min_priority: NotificationPriority,
}

fn default_parse_mode() -> String {
    "HTML".to_string()
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            enabled: false,
            bot_token: String::new(),
            chat_id: String::new(),
            parse_mode: default_parse_mode(),
            min_priority: NotificationPriority::Normal,
        }
    }
}

/// Telegram notification channel.
pub struct TelegramChannel {
    config: TelegramConfig,
    client: Client,
}

impl TelegramChannel {
    /// Create a new Telegram channel.
    pub fn new(config: TelegramConfig) -> Self {
        crate::utils::http_client::install_rustls_provider();
        Self {
            config,
            client: Client::new(),
        }
    }

    /// Build the message text for an event.
    fn build_message(&self, event: &NotificationEvent) -> String {
        let emoji = match event.priority() {
            NotificationPriority::Low => "\u{2139}\u{fe0f}", // ℹ️
            NotificationPriority::Normal => "\u{1f514}",     // 🔔
            NotificationPriority::High => "\u{26a0}\u{fe0f}", // ⚠️
            NotificationPriority::Critical => "\u{1f6a8}",   // 🚨
        };

        let title = event.title();
        let description = event.description();
        let priority = event.priority().to_string();
        let event_type = event.event_type().to_string();

        let text = if self.config.parse_mode.eq_ignore_ascii_case("HTML") {
            let escaped_title = escape_telegram_html(&title);
            let escaped_description = escape_telegram_html(&description);
            let escaped_priority = escape_telegram_html(&priority);
            let escaped_event_type = escape_telegram_html(&event_type);
            format!(
                "{emoji} <b>{escaped_title}</b>\n\n{escaped_description}\n\n<i>Priority: {escaped_priority} | Type: {escaped_event_type}</i>"
            )
        } else {
            format!(
                "{emoji} *{title}*\n\n{description}\n\n_Priority: {priority} | Type: {event_type}_"
            )
        };

        truncate_message(&text, TELEGRAM_MESSAGE_LIMIT)
    }

    /// Send request with rate limit handling.
    async fn send_with_retry(&self, payload: &serde_json::Value) -> Result<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.config.bot_token
        );
        let mut attempts = 0;

        loop {
            attempts += 1;

            let response = self
                .client
                .post(&url)
                .json(payload)
                .send()
                .await
                .map_err(|e| crate::Error::Other(format!("Telegram request failed: {}", e)))?;

            let status = response.status();

            if status.is_success() {
                return Ok(());
            }

            if status.as_u16() == 429 {
                let body: serde_json::Value = response.json().await.unwrap_or_default();

                let retry_after = body
                    .get("parameters")
                    .and_then(|p| p.get("retry_after"))
                    .and_then(|v| v.as_u64())
                    .map(Duration::from_secs);

                if attempts >= MAX_RATE_LIMIT_RETRIES {
                    warn!(
                        "Telegram rate limit: max retries ({}) exceeded, last retry_after was {:?}",
                        MAX_RATE_LIMIT_RETRIES, retry_after
                    );
                    return Err(crate::Error::Other(format!(
                        "Telegram rate limit exceeded after {} retries",
                        MAX_RATE_LIMIT_RETRIES
                    )));
                }

                let wait_duration = retry_after.unwrap_or(Duration::from_secs(1));
                debug!(
                    "Telegram rate limited (429), waiting {:?} before retry (attempt {}/{})",
                    wait_duration, attempts, MAX_RATE_LIMIT_RETRIES
                );
                tokio::time::sleep(wait_duration).await;
                continue;
            }

            // Other error
            let body = response.text().await.unwrap_or_default();
            warn!("Telegram sendMessage failed: {} - {}", status, body);
            return Err(crate::Error::Other(format!(
                "Telegram sendMessage failed: {} - {}",
                status, body
            )));
        }
    }
}

#[async_trait]
impl NotificationChannel for TelegramChannel {
    fn channel_type(&self) -> &'static str {
        "telegram"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled && !self.config.bot_token.is_empty() && !self.config.chat_id.is_empty()
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Check priority filter
        if event.priority() < self.config.min_priority {
            debug!(
                "Skipping Telegram notification for {} (priority {} < {})",
                event.event_type(),
                event.priority(),
                self.config.min_priority
            );
            return Ok(());
        }

        let text = self.build_message(event);
        let payload = json!({
            "chat_id": self.config.chat_id,
            "text": text,
            "parse_mode": self.config.parse_mode,
        });

        self.send_with_retry(&payload).await?;

        debug!("Telegram notification sent: {}", event.event_type());
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

fn escape_telegram_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Truncate a message to fit within the Telegram character limit.
fn truncate_message(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let suffix = "\n\n[truncated]";
    let budget = limit - suffix.len();
    let truncated: String = text.chars().take(budget).collect();
    format!("{truncated}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_config_default() {
        let config = TelegramConfig::default();
        assert!(!config.enabled);
        assert!(config.bot_token.is_empty());
        assert!(config.chat_id.is_empty());
        assert_eq!(config.parse_mode, "HTML");
        assert_eq!(config.min_priority, NotificationPriority::Normal);
    }

    #[test]
    fn test_telegram_channel_disabled() {
        let config = TelegramConfig::default();
        let channel = TelegramChannel::new(config);
        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_telegram_channel_enabled() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: "123:ABC".to_string(),
            chat_id: "456".to_string(),
            ..Default::default()
        };
        let channel = TelegramChannel::new(config);
        assert!(channel.is_enabled());
    }

    #[test]
    fn test_build_message_html() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: "tok".to_string(),
            chat_id: "123".to_string(),
            parse_mode: "HTML".to_string(),
            ..Default::default()
        };
        let channel = TelegramChannel::new(config);

        let event = NotificationEvent::SystemStartup {
            version: "1.0.0".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let msg = channel.build_message(&event);
        assert!(msg.contains("<b>"));
        assert!(msg.contains("1.0.0"));
    }

    #[test]
    fn test_build_message_html_escapes_dynamic_content() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: "tok".to_string(),
            chat_id: "123".to_string(),
            parse_mode: "html".to_string(),
            ..Default::default()
        };
        let channel = TelegramChannel::new(config);
        let event = NotificationEvent::StreamOnline {
            streamer_id: "sid".to_string(),
            streamer_name: "Alice <admin>".to_string(),
            title: "live <script>alert(1)</script> & chill".to_string(),
            category: Some("a<b".to_string()),
            timestamp: chrono::Utc::now(),
        };

        let msg = channel.build_message(&event);

        assert!(msg.contains("&lt;admin&gt;"));
        assert!(msg.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(msg.contains("&amp; chill"));
    }

    #[test]
    fn test_truncate_message() {
        let short = "hello";
        assert_eq!(truncate_message(short, 100), "hello");

        let long: String = "a".repeat(5000);
        let truncated = truncate_message(&long, TELEGRAM_MESSAGE_LIMIT);
        assert!(truncated.chars().count() <= TELEGRAM_MESSAGE_LIMIT);
        assert!(truncated.ends_with("[truncated]"));
    }
}
