//! Twitch danmu (chat) provider.
//!
//! Implements danmu collection for the Twitch streaming platform using IRC.

use async_trait::async_trait;
use regex::Regex;
use std::sync::OnceLock;

use crate::danmu::{DanmuConnection, DanmuMessage, DanmuProvider, DanmuType};
use crate::error::{Error, Result};

/// Twitch danmu provider using IRC protocol.
pub struct TwitchDanmuProvider {
    /// Regex for extracting channel name from URL
    url_regex: OnceLock<Regex>,
}

impl TwitchDanmuProvider {
    /// Create a new Twitch danmu provider.
    pub fn new() -> Self {
        Self {
            url_regex: OnceLock::new(),
        }
    }

    fn get_url_regex(&self) -> &Regex {
        self.url_regex.get_or_init(|| {
            Regex::new(r"(?:https?://)?(?:www\.)?twitch\.tv/([a-zA-Z0-9_]+)").unwrap()
        })
    }
}

impl Default for TwitchDanmuProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DanmuProvider for TwitchDanmuProvider {
    fn platform(&self) -> &str {
        "twitch"
    }

    async fn connect(&self, room_id: &str) -> Result<DanmuConnection> {
        // TODO: Implement actual IRC connection to Twitch chat
        // For now, create a connection handle that will be used when IRC is implemented
        let connection_id = format!("twitch-{}-{}", room_id, uuid::Uuid::new_v4());
        let mut connection = DanmuConnection::new(connection_id, "twitch", room_id);
        
        // In actual implementation:
        // 1. Connect to irc.chat.twitch.tv:6697 (TLS) or 6667 (plain)
        // 2. Send PASS oauth:<token> (anonymous: PASS oauth:)
        // 3. Send NICK justinfan<random> (anonymous) or actual username
        // 4. Send CAP REQ :twitch.tv/tags twitch.tv/commands
        // 5. Send JOIN #<channel>
        // 6. Handle PING/PONG for keepalive
        
        connection.set_connected();
        Ok(connection)
    }

    async fn disconnect(&self, connection: &mut DanmuConnection) -> Result<()> {
        // TODO: Implement actual IRC disconnection
        // 1. Send PART #<channel>
        // 2. Send QUIT
        // 3. Close connection
        
        connection.set_disconnected();
        Ok(())
    }

    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuMessage>> {
        if !connection.is_connected {
            return Err(Error::DanmuError("Connection is not active".to_string()));
        }

        // TODO: Implement actual message receiving from IRC
        // 1. Read next line from IRC connection
        // 2. Parse IRC message format
        // 3. Handle PING (respond with PONG)
        // 4. Parse PRIVMSG for chat messages
        // 5. Convert to DanmuMessage
        
        // Placeholder: return None to indicate no message available
        Ok(None)
    }

    fn supports_url(&self, url: &str) -> bool {
        self.get_url_regex().is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        self.get_url_regex()
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_lowercase())
    }
}

/// Parse a Twitch IRC message into a DanmuMessage.
#[allow(dead_code)]
fn parse_twitch_irc_message(line: &str) -> Result<Option<DanmuMessage>> {
    // Twitch IRC format with tags:
    // @badge-info=;badges=;color=#FF0000;display-name=User;emotes=;id=xxx;mod=0;room-id=123;
    // subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=456;user-type= 
    // :user!user@user.tmi.twitch.tv PRIVMSG #channel :message content
    
    if line.starts_with("PING") {
        // This is a PING, not a chat message
        return Ok(None);
    }

    if !line.contains("PRIVMSG") {
        // Not a chat message
        return Ok(None);
    }

    // Parse tags
    let mut tags = std::collections::HashMap::new();
    let mut remaining = line;
    
    if line.starts_with('@') {
        if let Some(space_idx) = line.find(' ') {
            let tag_str = &line[1..space_idx];
            for tag in tag_str.split(';') {
                if let Some(eq_idx) = tag.find('=') {
                    let key = &tag[..eq_idx];
                    let value = &tag[eq_idx + 1..];
                    tags.insert(key.to_string(), value.to_string());
                }
            }
            remaining = &line[space_idx + 1..];
        }
    }

    // Parse the rest: :user!user@user.tmi.twitch.tv PRIVMSG #channel :message
    let parts: Vec<&str> = remaining.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return Err(Error::DanmuError("Invalid IRC message format".to_string()));
    }

    let prefix = parts[0];
    let _command = parts[1]; // PRIVMSG
    let _channel = parts[2]; // #channel
    let content = if parts[3].starts_with(':') {
        &parts[3][1..]
    } else {
        parts[3]
    };

    // Extract username from prefix
    let username = prefix
        .strip_prefix(':')
        .and_then(|s| s.split('!').next())
        .unwrap_or("unknown");

    let display_name = tags
        .get("display-name")
        .cloned()
        .unwrap_or_else(|| username.to_string());
    
    let user_id = tags
        .get("user-id")
        .cloned()
        .unwrap_or_else(|| username.to_string());
    
    let message_id = tags
        .get("id")
        .cloned()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut msg = DanmuMessage::chat(message_id, user_id, display_name, content.trim());
    
    // Add color if present
    if let Some(color) = tags.get("color") {
        if !color.is_empty() {
            msg = msg.with_metadata("color", serde_json::json!(color));
        }
    }

    // Add badges if present
    if let Some(badges) = tags.get("badges") {
        if !badges.is_empty() {
            msg = msg.with_metadata("badges", serde_json::json!(badges));
        }
    }

    // Check for bits (cheering)
    if let Some(bits) = tags.get("bits") {
        msg.message_type = DanmuType::Gift;
        msg = msg.with_metadata("bits", serde_json::json!(bits.parse::<u32>().unwrap_or(0)));
    }

    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url() {
        let provider = TwitchDanmuProvider::new();
        
        assert!(provider.supports_url("https://www.twitch.tv/streamer"));
        assert!(provider.supports_url("http://twitch.tv/another_streamer"));
        assert!(provider.supports_url("twitch.tv/test123"));
        
        assert!(!provider.supports_url("https://www.huya.com/12345"));
        assert!(!provider.supports_url("https://www.youtube.com/watch?v=xxx"));
    }

    #[test]
    fn test_extract_room_id() {
        let provider = TwitchDanmuProvider::new();
        
        assert_eq!(
            provider.extract_room_id("https://www.twitch.tv/Streamer"),
            Some("streamer".to_string()) // lowercase
        );
        assert_eq!(
            provider.extract_room_id("http://twitch.tv/another_streamer"),
            Some("another_streamer".to_string())
        );
        assert_eq!(
            provider.extract_room_id("https://www.huya.com/12345"),
            None
        );
    }

    #[test]
    fn test_parse_twitch_irc_message() {
        let line = "@badge-info=;badges=broadcaster/1;color=#FF0000;display-name=TestUser;emotes=;id=abc123;mod=0;room-id=12345;subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=67890;user-type= :testuser!testuser@testuser.tmi.twitch.tv PRIVMSG #channel :Hello world!";
        
        let result = parse_twitch_irc_message(line).unwrap();
        assert!(result.is_some());
        
        let msg = result.unwrap();
        assert_eq!(msg.username, "TestUser");
        assert_eq!(msg.user_id, "67890");
        assert_eq!(msg.content, "Hello world!");
        assert_eq!(msg.message_type, DanmuType::Chat);
    }

    #[test]
    fn test_parse_ping_message() {
        let line = "PING :tmi.twitch.tv";
        let result = parse_twitch_irc_message(line).unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_connect_disconnect() {
        let provider = TwitchDanmuProvider::new();
        
        let mut connection = provider.connect("streamer").await.unwrap();
        assert!(connection.is_connected);
        assert_eq!(connection.platform, "twitch");
        assert_eq!(connection.room_id, "streamer");
        
        provider.disconnect(&mut connection).await.unwrap();
        assert!(!connection.is_connected);
    }
}
