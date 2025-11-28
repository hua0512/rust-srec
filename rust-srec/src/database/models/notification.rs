//! Notification database models.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Notification channel database model.
/// Represents a configured destination for system event notifications.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NotificationChannelDbModel {
    pub id: String,
    pub name: String,
    /// Channel type: Discord, Email, Webhook
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
#[strum(serialize_all = "PascalCase")]
#[serde(rename_all = "PascalCase")]
pub enum ChannelType {
    Discord,
    Email,
    Webhook,
}

impl ChannelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discord => "Discord",
            Self::Email => "Email",
            Self::Webhook => "Webhook",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Discord" => Some(Self::Discord),
            "Email" => Some(Self::Email),
            "Webhook" => Some(Self::Webhook),
            _ => None,
        }
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
    /// ISO 8601 timestamp of the first delivery attempt
    pub first_attempt_at: String,
    /// ISO 8601 timestamp of the final failed attempt
    pub last_attempt_at: String,
    /// ISO 8601 timestamp when added to dead letter queue
    pub created_at: String,
}

impl NotificationDeadLetterDbModel {
    pub fn new(
        channel_id: impl Into<String>,
        event_name: impl Into<String>,
        event_payload: impl Into<String>,
        error_message: impl Into<String>,
        retry_count: i32,
        first_attempt_at: impl Into<String>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            channel_id: channel_id.into(),
            event_name: event_name.into(),
            event_payload: event_payload.into(),
            error_message: error_message.into(),
            retry_count,
            first_attempt_at: first_attempt_at.into(),
            last_attempt_at: now.clone(),
            created_at: now,
        }
    }
}

/// System event names for notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
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
            "2024-01-01T00:00:00Z",
        );
        assert_eq!(entry.retry_count, 3);
    }
}
