//! Telegram Bot API notification channel.
//!
//! Sends messages via the Telegram Bot API (`POST /bot<token>/sendMessage`).
//! Handles 429 rate limits by respecting the `parameters.retry_after` field
//! returned in the JSON response body.

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

use super::NotificationChannel;
use super::http::{DEFAULT_TIMEOUT, HttpDelivery, RetryPolicy};
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

/// Conservative text budget in UTF-16 units (Telegram entity offsets use this unit).
const TELEGRAM_MESSAGE_LIMIT: usize = 4096;
const TRUNCATION_SUFFIX: &str = "\n\n[truncated]";

/// Telegram channel configuration.
#[derive(Clone, Serialize, Deserialize)]
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
    /// Formatting style (HTML, Markdown, MarkdownV2, or empty for plain text).
    /// Formatted styles use explicit Telegram entities so content always remains literal.
    #[serde(default = "default_parse_mode")]
    pub parse_mode: String,
    /// Minimum priority level to send (default: Normal).
    #[serde(default)]
    pub min_priority: NotificationPriority,
    /// Language for the rendered title and body; `None` follows the process-wide locale.
    /// See `notification::service::parse_channel_locale`.
    #[serde(default)]
    pub locale: Option<String>,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("parse_mode", &self.parse_mode)
            .field("min_priority", &self.min_priority)
            .field("locale", &self.locale)
            .finish()
    }
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
            locale: None,
        }
    }
}

/// Telegram notification channel.
pub struct TelegramChannel {
    config: TelegramConfig,
    http: HttpDelivery,
}

impl TelegramChannel {
    /// Create a new Telegram channel.
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            http: HttpDelivery::new("telegram", DEFAULT_TIMEOUT),
        }
    }

    /// Build literal text plus entities, which Telegram accepts instead of parse_mode.
    /// https://core.telegram.org/bots/api#sendmessage
    fn build_payload(&self, event: &NotificationEvent) -> Result<serde_json::Value> {
        let formatted = if self.config.parse_mode.is_empty() {
            false
        } else if ["HTML", "Markdown", "MarkdownV2"]
            .iter()
            .any(|mode| self.config.parse_mode.eq_ignore_ascii_case(mode))
        {
            true
        } else {
            return Err(crate::Error::config(
                "Unsupported Telegram parse mode; expected HTML, Markdown, MarkdownV2, or empty",
            ));
        };
        let emoji = match event.priority() {
            NotificationPriority::Low => "\u{2139}\u{fe0f}", // ℹ️
            NotificationPriority::Normal => "\u{1f514}",     // 🔔
            NotificationPriority::High => "\u{26a0}\u{fe0f}", // ⚠️
            NotificationPriority::Critical => "\u{1f6a8}",   // 🚨
        };

        let title = event.title_for(self.config.locale.as_deref());
        let description = event.description_for(self.config.locale.as_deref());
        let priority = event.priority().to_string();
        let event_type = event.event_type().to_string();

        let footer = format!("Priority: {priority} | Type: {event_type}");
        let title_offset = emoji.encode_utf16().count() + 1;
        let title_length = title.encode_utf16().count();
        let footer_offset = title_offset + title_length + 4 + description.encode_utf16().count();
        let footer_length = footer.encode_utf16().count();
        let full_text = format!("{emoji} {title}\n\n{description}\n\n{footer}");
        let (text, retained_units) = truncate_message(&full_text, TELEGRAM_MESSAGE_LIMIT);
        let mut payload = json!({ "chat_id": self.config.chat_id, "text": text });
        if formatted {
            let mut entities = Vec::new();
            for (kind, offset, length) in [
                ("bold", title_offset, title_length),
                ("italic", footer_offset, footer_length),
            ] {
                let end = (offset + length).min(retained_units);
                if end > offset {
                    entities.push(json!({"type": kind, "offset": offset, "length": end - offset}));
                }
            }
            payload["entities"] = json!(entities);
        }
        Ok(payload)
    }

    async fn send_with_retry(&self, payload: &serde_json::Value) -> Result<()> {
        let request = self
            .http
            .request(
                Method::POST,
                &format!(
                    "https://api.telegram.org/bot{}/sendMessage",
                    self.config.bot_token
                ),
            )?
            .json(payload);
        self.http.send(request, RetryPolicy::Telegram).await
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

        let payload = self.build_payload(event)?;

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

fn prefix_utf16(text: &str, budget: usize) -> (&str, usize) {
    let mut used = 0;
    let mut end = 0;
    for (offset, ch) in text.char_indices() {
        if used + ch.len_utf16() > budget {
            break;
        }
        used += ch.len_utf16();
        end = offset + ch.len_utf8();
    }
    (&text[..end], used)
}

/// Return bounded plain text and the retained original-text length. Entities never include
/// the truncation suffix, and every offset/length stays on a complete Unicode scalar.
fn truncate_message(text: &str, limit: usize) -> (String, usize) {
    let (prefix, units) = prefix_utf16(text, limit);
    if prefix.len() == text.len() {
        return (text.to_string(), units);
    }
    let (suffix, suffix_units) = prefix_utf16(TRUNCATION_SUFFIX, limit);
    let (prefix, retained_units) = prefix_utf16(text, limit - suffix_units);
    (format!("{prefix}{suffix}"), retained_units)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn channel(mode: &str, locale: &str) -> TelegramChannel {
        TelegramChannel::new(TelegramConfig {
            parse_mode: mode.to_string(),
            locale: Some(locale.to_string()),
            ..Default::default()
        })
    }

    fn event(name: &str, title: &str) -> NotificationEvent {
        NotificationEvent::StreamOnline {
            streamer_id: "streamer".to_string(),
            streamer_name: name.to_string(),
            title: title.to_string(),
            category: None,
            timestamp: chrono::Utc::now(),
        }
    }

    fn entity_text(payload: &serde_json::Value, index: usize) -> String {
        let units: Vec<_> = payload["text"].as_str().unwrap().encode_utf16().collect();
        let entity = &payload["entities"][index];
        let offset = entity["offset"].as_u64().unwrap() as usize;
        let length = entity["length"].as_u64().unwrap() as usize;
        assert!(length > 0);
        String::from_utf16(&units[offset..offset + length])
            .expect("entity spans complete characters")
    }

    #[test]
    fn supported_modes_preserve_literal_content_and_utf16_entities() {
        let special = "编😀 _*[]()~`>#+-=|{}.!\\ <b>&quot;";
        let event = event(special, special);
        for locale in ["en", "zh-CN"] {
            let expected_title = event.title_for(Some(locale));
            let baseline = channel("HTML", locale).build_payload(&event).unwrap();
            for mode in [
                "HTML",
                "html",
                "Markdown",
                "markdown",
                "MarkdownV2",
                "markdownv2",
            ] {
                let payload = channel(mode, locale).build_payload(&event).unwrap();
                assert_eq!(payload, baseline);
                assert!(
                    payload.get("parse_mode").is_none(),
                    "text must never be parsed as markup"
                );
                assert!(payload["text"].as_str().unwrap().contains(special));
                assert_eq!(payload["entities"].as_array().unwrap().len(), 2);
                assert_eq!(payload["entities"][0]["type"], "bold");
                assert_eq!(entity_text(&payload, 0), expected_title);
                assert_eq!(payload["entities"][1]["type"], "italic");
                assert_eq!(
                    entity_text(&payload, 1),
                    "Priority: normal | Type: stream_online"
                );
            }
        }
    }

    #[test]
    fn long_titles_and_bodies_clip_entities_before_truncation_marker() {
        let long = "😀中文<&_*[]\\".repeat(1000);
        for event in [event(&long, "short body"), event("short name", &long)] {
            for mode in ["HTML", "Markdown", "MarkdownV2"] {
                let payload = channel(mode, "en").build_payload(&event).unwrap();
                let text = payload["text"].as_str().unwrap();
                assert!(text.encode_utf16().count() <= TELEGRAM_MESSAGE_LIMIT);
                assert!(text.ends_with(TRUNCATION_SUFFIX));
                let retained_units = text
                    .strip_suffix(TRUNCATION_SUFFIX)
                    .unwrap()
                    .encode_utf16()
                    .count();
                for (index, entity) in payload["entities"].as_array().unwrap().iter().enumerate() {
                    let end =
                        entity["offset"].as_u64().unwrap() + entity["length"].as_u64().unwrap();
                    assert!(end as usize <= retained_units);
                    assert!(!entity_text(&payload, index).is_empty());
                }
            }
        }
    }

    #[test]
    fn empty_mode_is_plain_and_unknown_modes_fail_locally() {
        let event = event("Alice", "live");
        let plain = channel("", "en").build_payload(&event).unwrap();
        let formatted = channel("HTML", "en").build_payload(&event).unwrap();
        assert_eq!(plain["text"], formatted["text"]);
        assert!(plain.get("entities").is_none());
        assert!(plain.get("parse_mode").is_none());
        let error = channel("private-mode-value", "en")
            .build_payload(&event)
            .unwrap_err();
        assert!(matches!(error, crate::Error::Configuration(_)));
        assert!(!error.to_string().contains("private-mode-value"));
    }

    #[test]
    fn truncation_preserves_utf8_scalars_and_handles_small_budgets() {
        assert_eq!(truncate_message("hello", 5), ("hello".to_string(), 5));
        assert_eq!(truncate_message("😀中", 3), ("😀中".to_string(), 3));
        for budget in 0..40 {
            let (text, retained) = truncate_message(&"😀e\u{301}中文<&_*\\".repeat(20), budget);
            assert!(text.encode_utf16().count() <= budget);
            assert!(retained <= text.encode_utf16().count());
            assert!(String::from_utf16(&text.encode_utf16().collect::<Vec<_>>()).is_ok());
        }
        let (text, retained) = truncate_message(&"😀".repeat(3000), TELEGRAM_MESSAGE_LIMIT);
        assert!(text.ends_with(TRUNCATION_SUFFIX));
        assert_eq!(retained % 2, 0, "no surrogate pair is split");
    }
}
