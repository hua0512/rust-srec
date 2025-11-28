//! Danmu provider trait and common types.
//!
//! Defines the interface for platform-specific danmu providers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Result;

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
            timestamp: Utc::now(),
            message_type: DanmuType::Gift,
            metadata: Some(metadata),
        }
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

/// Connection handle for an active danmu stream.
#[derive(Debug)]
pub struct DanmuConnection {
    /// Unique connection ID
    pub id: String,
    /// Platform identifier
    pub platform: String,
    /// Streamer URL or room ID
    pub room_id: String,
    /// Whether the connection is active
    pub is_connected: bool,
    /// Connection start time
    pub connected_at: DateTime<Utc>,
    /// Number of reconnection attempts
    pub reconnect_count: u32,
}

impl DanmuConnection {
    /// Create a new connection handle.
    pub fn new(id: impl Into<String>, platform: impl Into<String>, room_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            platform: platform.into(),
            room_id: room_id.into(),
            is_connected: false,
            connected_at: Utc::now(),
            reconnect_count: 0,
        }
    }

    /// Mark the connection as connected.
    pub fn set_connected(&mut self) {
        self.is_connected = true;
        self.connected_at = Utc::now();
    }

    /// Mark the connection as disconnected.
    pub fn set_disconnected(&mut self) {
        self.is_connected = false;
    }

    /// Increment reconnect count.
    pub fn increment_reconnect(&mut self) {
        self.reconnect_count += 1;
    }
}

/// Trait for platform-specific danmu providers.
#[async_trait]
pub trait DanmuProvider: Send + Sync {
    /// Get the platform name this provider handles.
    fn platform(&self) -> &str;

    /// Connect to the danmu stream for a room.
    async fn connect(&self, room_id: &str) -> Result<DanmuConnection>;

    /// Disconnect from the danmu stream.
    async fn disconnect(&self, connection: &mut DanmuConnection) -> Result<()>;

    /// Receive the next danmu message.
    /// Returns None if the connection is closed.
    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuMessage>>;

    /// Check if the provider supports the given URL.
    fn supports_url(&self, url: &str) -> bool;

    /// Extract room ID from a streamer URL.
    fn extract_room_id(&self, url: &str) -> Option<String>;
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

    #[test]
    fn test_danmu_connection() {
        let mut conn = DanmuConnection::new("conn1", "huya", "12345");
        
        assert!(!conn.is_connected);
        assert_eq!(conn.reconnect_count, 0);
        
        conn.set_connected();
        assert!(conn.is_connected);
        
        conn.set_disconnected();
        assert!(!conn.is_connected);
        
        conn.increment_reconnect();
        assert_eq!(conn.reconnect_count, 1);
    }
}
