//! Twitch danmu (chat) provider.
//!
//! Implements danmu collection for the Twitch streaming platform using IRC over WebSocket.

use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::Result;
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuMessage, DanmuType};

use super::URL_REGEX;

/// Twitch WebSocket IRC server URL
const TWITCH_WS_URL: &str = "wss://irc-ws.chat.twitch.tv:443";

/// Heartbeat interval - Twitch sends PING every ~5 minutes, we respond with PONG
/// We don't need to send heartbeat proactively, just respond to PING
const HEARTBEAT_INTERVAL_SECS: u64 = 300;

/// Twitch Danmu Protocol Implementation using WebSocket IRC
#[derive(Clone, Default)]
pub struct TwitchDanmuProtocol {
    /// Optional OAuth token for authenticated sessions
    oauth_token: Option<String>,
}

impl TwitchDanmuProtocol {
    /// Create a new TwitchDanmuProtocol instance (anonymous).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new TwitchDanmuProtocol with OAuth token for authenticated access.
    pub fn with_oauth(token: impl Into<String>) -> Self {
        Self {
            oauth_token: Some(token.into()),
        }
    }

    /// Generate random anonymous username
    fn generate_anonymous_nick() -> String {
        let random_num: u32 = rand::random::<u32>() % 100000;
        format!("justinfan{}", random_num)
    }

    /// Parse IRC message into DanmuMessage
    fn parse_irc_message(line: &str) -> Option<DanmuMessage> {
        if line.starts_with("PING") || !line.contains("PRIVMSG") {
            return None;
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

        // Parse: :user!user@user.tmi.twitch.tv PRIVMSG #channel :message
        let parts: Vec<&str> = remaining.splitn(4, ' ').collect();
        if parts.len() < 4 {
            return None;
        }

        let prefix = parts[0];
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
                msg = msg.with_color(color);
            }
        }

        // Add badges as metadata
        if let Some(badges) = tags.get("badges") {
            if !badges.is_empty() {
                msg = msg.with_metadata("badges", serde_json::json!(badges));
            }
        }

        // Check for bits (cheering) - change message type to Gift
        if let Some(bits) = tags.get("bits") {
            msg.message_type = DanmuType::Gift;
            msg = msg.with_metadata("bits", serde_json::json!(bits.parse::<u32>().unwrap_or(0)));
        }

        Some(msg)
    }
}

#[async_trait]
impl DanmuProtocol for TwitchDanmuProtocol {
    fn platform(&self) -> &str {
        "twitch"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        URL_REGEX
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_lowercase())
    }

    async fn websocket_url(&self, _room_id: &str) -> Result<String> {
        Ok(TWITCH_WS_URL.to_string())
    }

    fn cookies(&self) -> Option<String> {
        // Twitch doesn't use cookies for IRC WebSocket
        None
    }

    async fn handshake_messages(&self, room_id: &str) -> Result<Vec<Message>> {
        let mut messages = Vec::new();

        // Request Twitch capabilities for tags and commands
        messages.push(Message::Text(
            "CAP REQ :twitch.tv/tags twitch.tv/commands".into(),
        ));

        // Send PASS (OAuth token or empty for anonymous)
        let pass = if let Some(ref token) = self.oauth_token {
            if token.starts_with("oauth:") {
                format!("PASS {}", token)
            } else {
                format!("PASS oauth:{}", token)
            }
        } else {
            "PASS oauth:".to_string()
        };
        messages.push(Message::Text(pass.into()));

        // Send NICK
        let nick = if self.oauth_token.is_some() {
            // For authenticated, the nick should match the token owner
            // But for simplicity, we still use anonymous nick pattern
            Self::generate_anonymous_nick()
        } else {
            Self::generate_anonymous_nick()
        };
        messages.push(Message::Text(format!("NICK {}", nick).into()));

        // Join channel
        let channel = if room_id.starts_with('#') {
            room_id.to_string()
        } else {
            format!("#{}", room_id.to_lowercase())
        };
        messages.push(Message::Text(format!("JOIN {}", channel).into()));

        Ok(messages)
    }

    fn heartbeat_message(&self) -> Option<Message> {
        // Twitch IRC doesn't require proactive heartbeat
        // We just respond to PING with PONG in decode_message
        None
    }

    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
    }

    async fn decode_message(
        &self,
        message: &Message,
        _room_id: &str,
        tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuMessage>> {
        match message {
            Message::Text(text) => {
                let mut danmus = Vec::new();

                // Handle each line (Twitch may send multiple messages in one frame)
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // debug!("Twitch IRC: {}", trimmed);

                    // Handle PING - respond with PONG
                    if trimmed.starts_with("PING") {
                        let pong_data = trimmed.strip_prefix("PING ").unwrap_or(":tmi.twitch.tv");
                        let pong = format!("PONG {}", pong_data);
                        debug!("Sending PONG: {}", pong);
                        let _ = tx.send(Message::Text(pong.into())).await;
                        continue;
                    }

                    // Parse chat messages
                    if let Some(danmu) = Self::parse_irc_message(trimmed) {
                        danmus.push(danmu);
                    }
                }

                Ok(danmus)
            }
            Message::Binary(data) => {
                // Twitch IRC uses text, but handle binary just in case
                if let Ok(text) = String::from_utf8(data.to_vec()) {
                    // Recursively process as text
                    Box::pin(self.decode_message(&Message::Text(text.into()), _room_id, tx)).await
                } else {
                    Ok(vec![])
                }
            }
            Message::Ping(data) => {
                // Respond to WebSocket-level PING
                let _ = tx.send(Message::Pong(data.clone())).await;
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}

/// Twitch danmu provider type alias.
pub type TwitchDanmuProvider = WebSocketDanmuProvider<TwitchDanmuProtocol>;

/// Creates a new Twitch danmu provider (anonymous).
pub fn create_twitch_danmu_provider() -> TwitchDanmuProvider {
    WebSocketDanmuProvider::with_protocol(TwitchDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use crate::danmaku::ConnectionConfig;

    use super::*;

    #[test]
    fn test_supports_url() {
        let protocol = TwitchDanmuProtocol::new();

        assert!(protocol.supports_url("https://www.twitch.tv/streamer"));
        assert!(protocol.supports_url("http://twitch.tv/another_streamer"));
        assert!(protocol.supports_url("twitch.tv/test123"));

        assert!(!protocol.supports_url("https://www.huya.com/12345"));
        assert!(!protocol.supports_url("https://www.youtube.com/watch?v=xxx"));
    }

    #[test]
    fn test_extract_room_id() {
        let protocol = TwitchDanmuProtocol::new();

        assert_eq!(
            protocol.extract_room_id("https://www.twitch.tv/Streamer"),
            Some("streamer".to_string()) // lowercase
        );
        assert_eq!(
            protocol.extract_room_id("http://twitch.tv/another_streamer"),
            Some("another_streamer".to_string())
        );
        assert_eq!(protocol.extract_room_id("https://www.huya.com/12345"), None);
    }

    #[test]
    fn test_parse_irc_message() {
        let line = "@badge-info=;badges=broadcaster/1;color=#FF0000;display-name=TestUser;emotes=;id=abc123;mod=0;room-id=12345;subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=67890;user-type= :testuser!testuser@testuser.tmi.twitch.tv PRIVMSG #channel :Hello world!";

        let result = TwitchDanmuProtocol::parse_irc_message(line);
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
        let result = TwitchDanmuProtocol::parse_irc_message(line);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_bits_message() {
        let line = "@badge-info=;badges=bits/100;bits=100;color=#FF0000;display-name=Cheerer;emotes=;id=abc123;mod=0;room-id=12345;subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=67890;user-type= :cheerer!cheerer@cheerer.tmi.twitch.tv PRIVMSG #channel :cheer100 Great stream!";

        let result = TwitchDanmuProtocol::parse_irc_message(line);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert_eq!(msg.message_type, DanmuType::Gift);
        assert!(msg.metadata.is_some());
        let metadata = msg.metadata.unwrap();
        assert_eq!(metadata.get("bits").unwrap(), &serde_json::json!(100));
    }

    #[test]
    fn test_generate_anonymous_nick() {
        let nick = TwitchDanmuProtocol::generate_anonymous_nick();
        assert!(nick.starts_with("justinfan"));
        assert!(nick.len() > 9); // "justinfan" + at least 1 digit
    }

    /// Real integration test - connects to an actual Twitch channel
    /// Run with: cargo test --package platforms-parser twitch::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_real_connection() {
        use crate::danmaku::provider::DanmuProvider;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();

        let provider = create_twitch_danmu_provider();
        let channel = "dota2ti";
        println!("Connecting to Twitch channel: {}", channel);
        let connection = provider
            .connect(channel, ConnectionConfig::default())
            .await
            .expect("Failed to connect");
        println!("Connected to Twitch channel #{}", channel);

        // Receive messages for 60 seconds
        let start = std::time::Instant::now();
        let mut message_count = 0;

        while start.elapsed() < Duration::from_secs(60) {
            match provider.receive(&connection).await {
                Ok(Some(msg)) => {
                    println!("[{:?}] {}: {}", msg.message_type, msg.username, msg.content);
                    message_count += 1;
                }
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    println!("Error: {}", e);
                    break;
                }
            }
        }

        println!("Received {} messages", message_count);
    }
}
