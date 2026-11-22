//! Twitch danmu (chat) provider.
//!
//! Implements danmu collection for the Twitch streaming platform using IRC over WebSocket.

use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::Result;
use crate::danmaku::websocket::{
    DanmuProtocol, DanmuProtocolFactory, DanmuProtocolOutput, WebSocketDanmuProvider,
};
use crate::danmaku::{DanmuItem, DanmuMessage, DanmuType};

use super::URL_REGEX;
use crate::extractor::utils::capture_group_1;

/// Twitch WebSocket IRC server URL
const TWITCH_WS_URL: &str = "wss://irc-ws.chat.twitch.tv:443";

/// Heartbeat interval - Twitch sends PING every ~5 minutes, we respond with PONG
/// We don't need to send heartbeat proactively, just respond to PING
const HEARTBEAT_INTERVAL_SECS: u64 = 300;

/// Twitch Danmu Protocol Implementation using WebSocket IRC
#[derive(Clone, Default)]
pub struct TwitchDanmuProtocol;

impl TwitchDanmuProtocol {
    /// Create a new TwitchDanmuProtocol instance (anonymous).
    pub fn new() -> Self {
        Self
    }

    /// Generate random anonymous username
    fn generate_anonymous_nick() -> String {
        let random_num: u32 = rand::random::<u32>() % 100000;
        format!("justinfan{}", random_num)
    }

    /// Unescape IRCv3 tag value escapes (`\:`, `\s`, `\\`, `\r`, `\n`).
    fn unescape_irc_tag(val: &str) -> Cow<'_, str> {
        if !val.contains('\\') {
            return Cow::Borrowed(val);
        }
        let mut out = String::with_capacity(val.len());
        let mut chars = val.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some(':') => out.push(';'),
                    Some('s') => out.push(' '),
                    Some('\\') => out.push('\\'),
                    Some('r') => out.push('\r'),
                    Some('n') => out.push('\n'),
                    Some(other) => out.push(other),
                    None => break,
                }
            } else {
                out.push(ch);
            }
        }
        Cow::Owned(out)
    }

    /// Parse IRC message into DanmuMessage
    fn parse_irc_message(line: &str) -> Option<DanmuMessage> {
        let mut tags: FxHashMap<&str, &str> = FxHashMap::default();
        let remaining = if let Some(rest) = line.strip_prefix('@') {
            let (tag_str, rem) = rest.split_once(' ')?;
            for tag in tag_str.split(';') {
                if let Some((key, value)) = tag.split_once('=') {
                    tags.insert(key, value);
                }
            }
            rem.trim_start_matches(' ')
        } else {
            line
        };

        let (prefix, remaining) = if let Some(rest) = remaining.strip_prefix(':') {
            let (pfx, rem) = rest.split_once(' ')?;
            (Some(pfx), rem.trim_start_matches(' '))
        } else {
            (None, remaining)
        };

        let (command, remaining) = remaining.split_once(' ')?;
        if command != "PRIVMSG" {
            return None;
        }

        let remaining = remaining.trim_start_matches(' ');
        let (_channel, trailing) = remaining.split_once(' ')?;
        let trailing = trailing.trim_start_matches(' ');
        let content = trailing.strip_prefix(':').unwrap_or(trailing);

        let username = prefix
            .and_then(|s| s.split('!').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown");

        let display_name = tags
            .get("display-name")
            .map(|s| Self::unescape_irc_tag(s))
            .filter(|s| !s.is_empty())
            .map_or_else(|| username.to_string(), Cow::into_owned);

        let user_id = tags
            .get("user-id")
            .map(|s| Self::unescape_irc_tag(s))
            .filter(|s| !s.is_empty())
            .map_or_else(|| username.to_string(), Cow::into_owned);

        let message_id = tags
            .get("id")
            .map(|s| Self::unescape_irc_tag(s))
            .filter(|s| !s.is_empty())
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), Cow::into_owned);

        let mut msg = DanmuMessage::chat(message_id, user_id, display_name, content.trim());

        // Add color if present
        if let Some(color) = tags.get("color").map(|s| Self::unescape_irc_tag(s))
            && !color.is_empty()
        {
            msg = msg.with_color(color.into_owned());
        }

        // Add badges as metadata
        if let Some(badges) = tags.get("badges").map(|s| Self::unescape_irc_tag(s))
            && !badges.is_empty()
        {
            msg = msg.with_metadata("badges", serde_json::json!(badges));
        }

        // Check for bits (cheering) - change message type to Gift
        if let Some(bits) = tags.get("bits").map(|s| Self::unescape_irc_tag(s)) {
            msg.message_type = DanmuType::Gift;
            msg = msg.with_metadata("bits", serde_json::json!(bits.parse::<u32>().unwrap_or(0)));
        }

        Some(msg)
    }
}

impl DanmuProtocolFactory for TwitchDanmuProtocol {
    type Protocol = Self;

    fn platform(&self) -> &str {
        "twitch"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        capture_group_1(&URL_REGEX, url).map(str::to_lowercase)
    }

    fn create_protocol(&self) -> Self::Protocol {
        self.clone()
    }
}

impl DanmuProtocol for TwitchDanmuProtocol {
    async fn websocket_url(&mut self, _room_id: &str) -> Result<String> {
        Ok(TWITCH_WS_URL.to_string())
    }

    fn cookies(&self) -> Option<String> {
        // Twitch doesn't use cookies for IRC WebSocket
        None
    }

    fn send_cookie_header(&self) -> bool {
        false
    }

    async fn handshake_messages(&mut self, room_id: &str) -> Result<Vec<Message>> {
        let mut messages = Vec::new();

        // Request Twitch capabilities for tags and commands
        messages.push(Message::Text(
            "CAP REQ :twitch.tv/tags twitch.tv/commands".into(),
        ));

        messages.push(Message::Text("PASS oauth:".into()));
        let nick = Self::generate_anonymous_nick();
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
        &mut self,
        message: &Message,
        _room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        match message {
            Message::Text(text) => Ok(Self::decode_text(text)),
            Message::Binary(data) => {
                // Twitch IRC uses text, but handle binary just in case
                if let Ok(text) = std::str::from_utf8(data) {
                    Ok(Self::decode_text(text))
                } else {
                    Ok(DanmuProtocolOutput::default())
                }
            }
            Message::Ping(data) => Ok(DanmuProtocolOutput::outbound(vec![Message::Pong(
                data.clone(),
            )])),
            _ => Ok(DanmuProtocolOutput::default()),
        }
    }
}

impl TwitchDanmuProtocol {
    fn decode_text(text: &str) -> DanmuProtocolOutput {
        let mut items = Vec::new();
        let mut outbound = Vec::new();

        // Handle each line (Twitch may send multiple messages in one frame)
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Handle PING - respond with PONG
            if trimmed.starts_with("PING") {
                let pong_data = trimmed.strip_prefix("PING ").unwrap_or(":tmi.twitch.tv");
                let pong = format!("PONG {}", pong_data);
                debug!("Sending PONG: {}", pong);
                outbound.push(Message::Text(pong.into()));
                continue;
            }

            // Parse chat messages
            if let Some(danmu) = Self::parse_irc_message(trimmed) {
                items.push(DanmuItem::Message(danmu));
            }
        }

        DanmuProtocolOutput::new(items, outbound)
    }
}

/// Twitch danmu provider type alias.
pub type TwitchDanmuProvider = WebSocketDanmuProvider<TwitchDanmuProtocol>;

/// Creates a new Twitch danmu provider (anonymous).
pub fn create_twitch_danmu_provider() -> TwitchDanmuProvider {
    WebSocketDanmuProvider::with_factory(TwitchDanmuProtocol, None)
}

#[cfg(test)]
mod tests {
    use crate::danmaku::ConnectionConfig;

    use super::*;

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

    #[tokio::test]
    async fn ping_returns_pong_as_outbound_protocol_frame() {
        let mut protocol = TwitchDanmuProtocol;
        let output = protocol
            .decode_message(&Message::Text("PING :tmi.twitch.tv".into()), "channel")
            .await
            .expect("decode ping");
        let (items, outbound) = output.into_parts();

        assert!(items.is_empty());
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0], Message::Text("PONG :tmi.twitch.tv".into()));
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
    fn irc_parser_unescapes_ircv3_tags_and_rejects_non_privmsg() {
        let escaped = r"@display-name=Alice\sBob\:Jr\\III;id=msg-1;user-id=42 :alice!alice@alice.tmi.twitch.tv PRIVMSG #chan :hello :there";
        let msg = TwitchDanmuProtocol::parse_irc_message(escaped).expect("valid PRIVMSG");
        assert_eq!(msg.username, r"Alice Bob;Jr\III");
        assert_eq!(msg.user_id, "42");
        assert_eq!(msg.id, "msg-1");
        assert_eq!(msg.content, "hello :there");

        // A NOTICE or USERNOTICE whose trailing text mentions "PRIVMSG" must not be parsed as chat.
        let notice = ":tmi.twitch.tv NOTICE #chan :Do not spoof PRIVMSG #chan :fake";
        assert!(TwitchDanmuProtocol::parse_irc_message(notice).is_none());
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
        let mut items = provider
            .connect(channel, ConnectionConfig::default())
            .await
            .expect("Failed to connect")
            .items;
        println!("Connected to Twitch channel #{}", channel);

        // Receive messages for 60 seconds
        let start = std::time::Instant::now();
        let mut message_count = 0;

        while start.elapsed() < Duration::from_secs(60) {
            match tokio::time::timeout(Duration::from_millis(500), items.recv()).await {
                Ok(Some(item)) => match item {
                    crate::danmaku::DanmuItem::Message(msg) => {
                        println!("[{:?}] {}: {}", msg.message_type, msg.username, msg.content);
                        message_count += 1;
                    }
                    crate::danmaku::DanmuItem::Control(control) => {
                        println!("[control] {:?}", control);
                    }
                },
                Ok(None) => {
                    println!("Stream closed by provider");
                    break;
                }
                Err(_) => {
                    // No message within the window; keep waiting until the
                    // 60s budget is spent.
                }
            }
        }

        println!("Received {} messages", message_count);
    }
}
