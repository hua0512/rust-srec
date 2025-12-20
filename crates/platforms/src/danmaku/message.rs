//! Danmu message types.
//!
//! Core message structures for representing chat messages from streaming platforms.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type of danmu message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DanmuType {
    /// Regular chat message
    Chat,
    /// Gift/donation
    Gift,
    /// Super chat (paid highlighted message)
    SuperChat,
    /// System message
    System,
    /// User join notification
    UserJoin,
    /// User follow notification
    Follow,
    /// Subscription notification
    Subscription,
    /// Other platform-specific message type
    Other,
}

impl Default for DanmuType {
    fn default() -> Self {
        Self::Chat
    }
}

/// A single danmu message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DanmuMessage {
    /// Unique message ID (platform-specific)
    pub id: String,
    /// User ID of the sender
    pub user_id: String,
    /// Display name of the sender
    pub username: String,
    /// Message content
    pub content: String,
    /// Color of the message (hex string, optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Timestamp when the message was sent
    pub timestamp: DateTime<Utc>,
    /// Type of message
    pub message_type: DanmuType,
    /// Platform-specific metadata (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl DanmuMessage {
    /// Create a new chat message.
    pub fn chat(
        id: impl Into<String>,
        user_id: impl Into<String>,
        username: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            user_id: user_id.into(),
            username: username.into(),
            content: content.into(),
            color: None,
            timestamp: Utc::now(),
            message_type: DanmuType::Chat,
            metadata: None,
        }
    }

    /// Create a new gift message.
    pub fn gift(
        id: impl Into<String>,
        user_id: impl Into<String>,
        username: impl Into<String>,
        gift_name: impl Into<String>,
        gift_count: u32,
    ) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("gift_name".to_string(), serde_json::json!(gift_name.into()));
        metadata.insert("gift_count".to_string(), serde_json::json!(gift_count));

        Self {
            id: id.into(),
            user_id: user_id.into(),
            username: username.into(),
            content: String::new(),
            color: None,
            timestamp: Utc::now(),
            message_type: DanmuType::Gift,
            metadata: Some(metadata),
        }
    }

    /// Set the color of the message.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Add metadata to the message.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value);
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_danmu_message_chat() {
        let msg = DanmuMessage::chat("1", "user1", "TestUser", "Hello world!");

        assert_eq!(msg.id, "1");
        assert_eq!(msg.user_id, "user1");
        assert_eq!(msg.username, "TestUser");
        assert_eq!(msg.content, "Hello world!");
        assert_eq!(msg.message_type, DanmuType::Chat);
        assert!(msg.metadata.is_none());
    }

    #[test]
    fn test_danmu_message_gift() {
        let msg = DanmuMessage::gift("2", "user2", "GiftUser", "Rocket", 5);

        assert_eq!(msg.message_type, DanmuType::Gift);
        let metadata = msg.metadata.as_ref().unwrap();
        assert_eq!(metadata.get("gift_name").unwrap(), "Rocket");
        assert_eq!(metadata.get("gift_count").unwrap(), 5);
    }

    #[test]
    fn test_danmu_message_with_metadata() {
        let msg = DanmuMessage::chat("1", "user1", "Test", "Hi")
            .with_metadata("color", serde_json::json!("#FF0000"));

        let metadata = msg.metadata.as_ref().unwrap();
        assert_eq!(metadata.get("color").unwrap(), "#FF0000");
    }
}
