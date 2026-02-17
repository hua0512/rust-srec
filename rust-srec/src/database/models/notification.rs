//! Notification database models.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Notification channel database model.
/// Represents a configured destination for system event notifications.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelDbModel {
    pub id: String,
    pub name: String,
    /// Channel type: Discord, Email, Telegram, Webhook
    pub channel_type: String,
    /// JSON blob for channel-specific settings
    pub settings: String,
}

impl NotificationChannelDbModel {
    pub fn new(
        name: impl Into<String>,
        channel_type: ChannelType,
        settings: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            channel_type: channel_type.as_str().to_string(),
            settings: settings.into(),
        }
    }
}

/// Notification channel types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum ChannelType {
    Discord,
    Email,
    Telegram,
    Webhook,
}

impl ChannelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discord => "Discord",
            Self::Email => "Email",
            Self::Telegram => "Telegram",
            Self::Webhook => "Webhook",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "discord" => Some(Self::Discord),
            "email" => Some(Self::Email),
            "telegram" => Some(Self::Telegram),
            "webhook" => Some(Self::Webhook),
            _ => None,
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Discord channel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordChannelSettings {
    pub webhook_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// Telegram channel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramChannelSettings {
    pub bot_token: String,
    pub chat_id: String,
}

/// Email channel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailChannelSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_address: String,
    pub to_addresses: Vec<String>,
    #[serde(default)]
    pub use_tls: bool,
}

/// Webhook channel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookChannelSettings {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub min_priority: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
}

fn default_method() -> String {
    "POST".to_string()
}

/// Notification subscription database model.
/// Links a notification channel to events it should be notified about.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NotificationSubscriptionDbModel {
    pub channel_id: String,
    pub event_name: String,
}

/// Notification dead letter database model.
/// Stores notifications that failed all retry attempts.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NotificationDeadLetterDbModel {
    pub id: String,
    pub channel_id: String,
    pub event_name: String,
    /// JSON blob of the event payload
    pub event_payload: String,
    pub error_message: String,
    pub retry_count: i32,
    /// Unix epoch milliseconds (UTC) of the first delivery attempt.
    pub first_attempt_at: i64,
    /// Unix epoch milliseconds (UTC) of the final failed attempt.
    pub last_attempt_at: i64,
    /// Unix epoch milliseconds (UTC) when added to dead letter queue.
    pub created_at: i64,
}

impl NotificationDeadLetterDbModel {
    pub fn new(
        channel_id: impl Into<String>,
        event_name: impl Into<String>,
        event_payload: impl Into<String>,
        error_message: impl Into<String>,
        retry_count: i32,
        first_attempt_at: i64,
    ) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            channel_id: channel_id.into(),
            event_name: event_name.into(),
            event_payload: event_payload.into(),
            error_message: error_message.into(),
            retry_count,
            first_attempt_at,
            last_attempt_at: now,
            created_at: now,
        }
    }
}

/// Notification event log database model.
///
/// Stores a persistent history of notification events (independent of delivery success).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationEventLogDbModel {
    pub id: String,
    pub event_type: String,
    pub priority: String,
    /// JSON blob of the event payload
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streamer_id: Option<String>,
    /// Unix epoch milliseconds (UTC) when the event occurred.
    pub created_at: i64,
}

/// Web Push subscription database model.
///
/// Stores per-user browser push subscriptions (VAPID/Web Push).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WebPushSubscriptionDbModel {
    pub id: String,
    pub user_id: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    /// Minimum priority to send (low|normal|high|critical).
    pub min_priority: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional next allowed attempt time. When set and in the future, delivery is skipped.
    pub next_attempt_at: Option<i64>,
    /// Last time we received a 429 from the push service.
    pub last_429_at: Option<i64>,
}

/// System event names for notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemEvent {
    StreamOnline,
    StreamOffline,
    DownloadStarted,
    DownloadCompleted,
    DownloadError,
    FatalError,
    OutOfSpace,
    PipelineStarted,
    PipelineCompleted,
    PipelineFailed,
    PipelineQueueWarning,
    PipelineQueueCritical,
    DiskSpaceWarning,
    DiskSpaceCritical,
}

impl SystemEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StreamOnline => "StreamOnline",
            Self::StreamOffline => "StreamOffline",
            Self::DownloadStarted => "DownloadStarted",
            Self::DownloadCompleted => "DownloadCompleted",
            Self::DownloadError => "DownloadError",
            Self::FatalError => "FatalError",
            Self::OutOfSpace => "OutOfSpace",
            Self::PipelineStarted => "PipelineStarted",
            Self::PipelineCompleted => "PipelineCompleted",
            Self::PipelineFailed => "PipelineFailed",
            Self::PipelineQueueWarning => "PipelineQueueWarning",
            Self::PipelineQueueCritical => "PipelineQueueCritical",
            Self::DiskSpaceWarning => "DiskSpaceWarning",
            Self::DiskSpaceCritical => "DiskSpaceCritical",
        }
    }
}

impl std::fmt::Display for SystemEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_type() {
        assert_eq!(ChannelType::Discord.as_str(), "Discord");
        assert_eq!(ChannelType::parse("Email"), Some(ChannelType::Email));
    }

    #[test]
    fn test_discord_settings() {
        let settings = DiscordChannelSettings {
            webhook_url: "https://discord.com/api/webhooks/...".to_string(),
            username: Some("Bot".to_string()),
            avatar_url: None,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("webhook_url"));
    }

    #[test]
    fn test_dead_letter_new() {
        let entry = NotificationDeadLetterDbModel::new(
            "channel-1",
            "StreamOnline",
            r#"{"streamer":"test"}"#,
            "Connection refused",
            3,
            0,
        );
        assert_eq!(entry.retry_count, 3);
    }
}
