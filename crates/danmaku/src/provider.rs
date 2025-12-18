//! Danmu provider trait and connection types.
//!
//! Defines the interface for platform-specific danmu providers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::message::DanmuMessage;

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
    pub fn new(
        id: impl Into<String>,
        platform: impl Into<String>,
        room_id: impl Into<String>,
    ) -> Self {
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
