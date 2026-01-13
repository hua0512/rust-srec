//! TwitCasting danmu (chat/comment) provider.
//!
//! Implements danmu collection for the TwitCasting streaming platform using WebSocket.
//!
//! Protocol:
//! 1. Get movie ID from stream server API: GET https://twitcasting.tv/streamserver.php?target={userId}&mode=client
//! 2. Get WebSocket URL: POST https://twitcasting.tv/eventpubsuburl.php with movie_id and password
//! 3. Connect to the returned wss:// URL for real-time comments (JSON arrays)

use async_trait::async_trait;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuItem, DanmuMessage};
use crate::extractor::default::default_client;

use super::URL_REGEX;

/// TwitCasting stream server API (same as builder uses)
const STREAM_SERVER_API: &str = "https://twitcasting.tv/streamserver.php";

/// TwitCasting event pubsub URL endpoint
const EVENT_PUBSUB_URL: &str = "https://twitcasting.tv/eventpubsuburl.php";

/// Heartbeat interval - TwitCasting WebSocket uses ping/pong
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Response from streamserver.php API
#[derive(Debug, Deserialize)]
struct StreamServerResponse {
    movie: MovieInfo,
}

/// Movie information from streamserver.php
#[derive(Debug, Deserialize)]
struct MovieInfo {
    id: i64,
    #[serde(default)]
    live: bool,
}

/// Response from eventpubsuburl.php
#[derive(Debug, Deserialize)]
struct EventPubSubResponse {
    url: String,
}

/// Comment message received from WebSocket
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct TwitcastingComment {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, alias = "from_user", alias = "author")]
    from_user: Option<CommentUser>,
    #[serde(default, alias = "createdAt", alias = "created_at")]
    created_at: Option<i64>,
    #[serde(default, alias = "num_comments")]
    #[allow(dead_code)]
    num_comments: Option<u64>,
}

/// User info in comment
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct CommentUser {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, alias = "display_name")]
    display_name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "screen_id")]
    screen_name: Option<String>,
    #[serde(default, alias = "userId", alias = "userID")]
    user_id: Option<String>,
    #[serde(default, alias = "profile_image", alias = "image", alias = "avatar")]
    profile_image: Option<String>,
    #[serde(default)]
    grade: Option<i32>,
}

/// TwitCasting Danmu Protocol Implementation
#[derive(Clone)]
pub struct TwitcastingDanmuProtocol {
    client: Client,
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
    /// Optional password for password-protected streams
    password: Option<String>,
    /// Cached WebSocket URL (set after websocket_url is called)
    cached_ws_url: std::sync::Arc<parking_lot::RwLock<Option<String>>>,
}

impl Default for TwitcastingDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            cookies: None,
            password: None,
            cached_ws_url: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        }
    }
}

impl TwitcastingDanmuProtocol {
    /// Create a new TwitcastingDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new TwitcastingDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            client: default_client(),
            cookies: Some(cookies.into()),
            password: None,
            cached_ws_url: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Create a new TwitcastingDanmuProtocol with password for protected streams.
    pub fn with_password(password: impl Into<String>) -> Self {
        Self {
            client: default_client(),
            cookies: None,
            password: Some(password.into()),
            cached_ws_url: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Create a new TwitcastingDanmuProtocol with both cookies and password.
    pub fn with_auth(cookies: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            client: default_client(),
            cookies: Some(cookies.into()),
            password: Some(password.into()),
            cached_ws_url: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Get the movie ID for a user from streamserver.php API.
    async fn get_movie_id(&self, user_id: &str) -> Result<String> {
        // Build query params like the builder does
        let mut params = vec![
            ("target", user_id),
            ("mode", "client"),
            ("player", "pc_web"),
        ];

        // Add password hash if provided
        let pass_hash = self.password.as_ref().map(|pass| {
            let mut md5 = Md5::new();
            md5.update(pass.as_bytes());
            format!("{:x}", md5.finalize())
        });

        if let Some(ref hash) = pass_hash {
            params.push(("word", hash));
        }

        let response = self
            .client
            .get(STREAM_SERVER_API)
            .query(&params)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header("Referer", "https://twitcasting.tv/")
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get stream info: {}", e)))?;

        if !response.status().is_success() {
            return Err(DanmakuError::connection(format!(
                "Failed to get stream info: HTTP {}",
                response.status()
            )));
        }

        let data: StreamServerResponse = response.json().await.map_err(|e| {
            DanmakuError::protocol(format!("Failed to parse stream response: {}", e))
        })?;

        if !data.movie.live {
            warn!("TwitCasting stream is not live");
        }

        // Convert i64 movie ID to string
        Ok(data.movie.id.to_string())
    }

    /// Get WebSocket URL for event pubsub.
    async fn get_event_pubsub_url(&self, movie_id: &str) -> Result<String> {
        // Build form data with movie_id and optional password
        let password = self.password.as_deref().unwrap_or("");

        let request = self
            .client
            .post(EVENT_PUBSUB_URL)
            .header("Accept", "*/*")
            .header("Accept-Encoding", "gzip, deflate, br")
            .header("Cache-Control", "no-cache")
            .header("Referer", "https://twitcasting.tv/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .form(&[("movie_id", movie_id), ("password", password)])
            .timeout(std::time::Duration::from_secs(5));

        let response = request
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get pubsub URL: {}", e)))?;

        if !response.status().is_success() {
            return Err(DanmakuError::connection(format!(
                "Failed to get pubsub URL: HTTP {}",
                response.status()
            )));
        }

        let data: EventPubSubResponse = response.json().await.map_err(|e| {
            DanmakuError::protocol(format!("Failed to parse pubsub response: {}", e))
        })?;

        Ok(data.url)
    }

    /// Parse comments from JSON (can be array or single object).
    fn parse_comments(data: &str) -> Vec<DanmuMessage> {
        let mut danmus = Vec::new();

        // Try to parse as JSON array first
        match serde_json::from_str::<Vec<TwitcastingComment>>(data) {
            Ok(comments) => {
                for comment in comments {
                    if let Some(danmu) = Self::comment_to_danmu(&comment) {
                        danmus.push(danmu);
                    }
                }
            }
            Err(e) => {
                debug!("TwitCasting: failed to parse as array: {}", e);
                // Try single object fallback
                match serde_json::from_str::<TwitcastingComment>(data) {
                    Ok(comment) => {
                        if let Some(danmu) = Self::comment_to_danmu(&comment) {
                            danmus.push(danmu);
                        }
                    }
                    Err(e2) => {
                        debug!("TwitCasting: failed to parse as object: {}", e2);
                    }
                }
            }
        }

        danmus
    }

    /// Convert a single TwitcastingComment to DanmuMessage.
    fn comment_to_danmu(comment: &TwitcastingComment) -> Option<DanmuMessage> {
        // Skip if it is not a comment message (if type is present)
        if let Some(ref t) = comment.msg_type
            && (t != "comment" && t != "gift")
        {
            debug!("TwitCasting: skipping message type: {}", t);
            return None;
        }

        let message = match comment.message.as_ref() {
            Some(m) => m,
            None => {
                debug!("TwitCasting: message field is missing: {:?}", comment);
                return None;
            }
        };

        // Handle ID which can be a number or string in JSON
        let id = comment
            .id
            .as_ref()
            .map(|v| match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => v.to_string(),
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let (user_id, display_name, avatar) = if let Some(ref user) = comment.from_user {
            // Priority for display name: name -> display_name -> screen_name -> id -> Anonymous
            let name = user
                .name
                .clone()
                .or_else(|| user.display_name.clone())
                .or_else(|| user.screen_name.clone())
                .or_else(|| user.id.clone())
                .unwrap_or_else(|| "Anonymous".to_string());

            // Priority for user id: id -> user_id -> screen_name -> "unknown"
            let uid = user
                .id
                .clone()
                .or_else(|| user.user_id.clone())
                .or_else(|| user.screen_name.clone())
                .unwrap_or_else(|| "unknown".to_string());

            let av = user.profile_image.clone();
            (uid, name, av)
        } else {
            debug!(
                "TwitCasting: from_user/author info is missing: {:?}",
                comment
            );
            ("unknown".to_string(), "Anonymous".to_string(), None)
        };

        let mut msg = DanmuMessage::chat(&id, &user_id, &display_name, message);

        if let Some(av) = avatar {
            msg = msg.with_metadata("avatar", serde_json::json!(av));
        }
        if let Some(created_at) = comment.created_at {
            msg = msg.with_metadata("created_at", serde_json::json!(created_at));
        }

        Some(msg)
    }
}

#[async_trait]
impl DanmuProtocol for TwitcastingDanmuProtocol {
    fn platform(&self) -> &str {
        "twitcasting"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        URL_REGEX
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    async fn websocket_url(&self, room_id: &str) -> Result<String> {
        // First get the movie ID from streamserver.php
        let movie_id = self.get_movie_id(room_id).await?;
        debug!("TwitCasting movie ID: {}", movie_id);

        // Then get the WebSocket URL
        let ws_url = self.get_event_pubsub_url(&movie_id).await?;
        debug!("TwitCasting WebSocket URL: {}", ws_url);

        // Cache the URL
        *self.cached_ws_url.write() = Some(ws_url.clone());

        Ok(ws_url)
    }

    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    fn headers(&self, _room_id: &str) -> Vec<(String, String)> {
        vec![
            ("Origin".to_string(), "https://twitcasting.tv".to_string()),
            ("Referer".to_string(), "https://twitcasting.tv".to_string()),
        ]
    }

    async fn handshake_messages(&self, _room_id: &str) -> Result<Vec<Message>> {
        // TwitCasting doesn't require explicit handshake - connection is sufficient
        Ok(vec![])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        // TwitCasting uses WebSocket-level ping/pong
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
    ) -> Result<Vec<DanmuItem>> {
        match message {
            Message::Text(text) => {
                let mut items = Vec::new();

                // TwitCasting sends JSON arrays of comments
                for line in text.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    // debug!("TwitCasting raw message: {}", line);

                    let parsed = Self::parse_comments(line);
                    if parsed.is_empty() {
                        debug!("TwitCasting non-comment message: {}", line);
                    }
                    items.extend(parsed.into_iter().map(DanmuItem::Message));
                }

                Ok(items)
            }
            Message::Binary(data) => {
                // Try to parse binary as text
                if let Ok(text) = String::from_utf8(data.to_vec()) {
                    return Ok(Self::parse_comments(&text)
                        .into_iter()
                        .map(DanmuItem::Message)
                        .collect());
                }
                Ok(vec![])
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

/// TwitCasting danmu provider type alias.
pub type TwitcastingDanmuProvider = WebSocketDanmuProvider<TwitcastingDanmuProtocol>;

/// Creates a new TwitCasting danmu provider.
pub fn create_twitcasting_danmu_provider() -> TwitcastingDanmuProvider {
    WebSocketDanmuProvider::with_protocol(TwitcastingDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use crate::danmaku::ConnectionConfig;

    use super::*;

    #[test]
    fn test_supports_url() {
        let protocol = TwitcastingDanmuProtocol::new();

        assert!(protocol.supports_url("https://twitcasting.tv/username"));
        assert!(protocol.supports_url("http://twitcasting.tv/another_user"));
        assert!(protocol.supports_url("twitcasting.tv/test123"));
        assert!(protocol.supports_url("https://www.twitcasting.tv/user-name"));

        assert!(!protocol.supports_url("https://www.twitch.tv/user"));
        assert!(!protocol.supports_url("https://www.youtube.com/watch?v=xxx"));
    }

    #[test]
    fn test_extract_room_id() {
        let protocol = TwitcastingDanmuProtocol::new();

        assert_eq!(
            protocol.extract_room_id("https://twitcasting.tv/nodasori2525"),
            Some("nodasori2525".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("http://twitcasting.tv/another_user"),
            Some("another_user".to_string())
        );
        assert_eq!(
            protocol.extract_room_id("https://www.twitcasting.tv/user-name"),
            Some("user-name".to_string())
        );
        assert_eq!(protocol.extract_room_id("https://www.twitch.tv/user"), None);
    }

    #[test]
    fn test_parse_comment_array() {
        let json = r#"[{"message":"Hello World!","fromUser":{"name":"TestUser","id":"user123"}}]"#;

        let result = TwitcastingDanmuProtocol::parse_comments(json);
        assert_eq!(result.len(), 1);

        let msg = &result[0];
        assert_eq!(msg.content, "Hello World!");
        assert_eq!(msg.username, "TestUser");
        assert_eq!(msg.user_id, "user123");
    }

    #[test]
    fn test_parse_complex_comment() {
        let json = r#"[{"type":"comment","id":32613491398,"message":"\u304a","createdAt":1766275295000,"author":{"id":"user123","name":"User","screenName":"g:123","profileImage":"..."},"numComments":30665}]"#;
        let result = TwitcastingDanmuProtocol::parse_comments(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].username, "User");
        assert_eq!(result[0].user_id, "user123");
        assert_eq!(result[0].id, "32613491398");
    }

    #[test]
    fn test_parse_multiple_comments() {
        let json = r#"[{"message":"First"},{"message":"Second"}]"#;

        let result = TwitcastingDanmuProtocol::parse_comments(json);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "First");
        assert_eq!(result[1].content, "Second");
    }

    #[test]
    fn test_parse_invalid_json() {
        let json = r#"not valid json"#;

        let result = TwitcastingDanmuProtocol::parse_comments(json);
        assert!(result.is_empty());
    }

    /// Real integration test - connects to an actual TwitCasting stream
    /// Run with: cargo test --package platforms-parser twitcasting::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_real_connection() {
        use crate::danmaku::provider::DanmuProvider;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();

        let provider = create_twitcasting_danmu_provider();
        let user_id = "icchy8591";

        println!("Connecting to TwitCasting user: {}", user_id);

        match provider.connect(user_id, ConnectionConfig::default()).await {
            Ok(connection) => {
                println!("Connected!");

                // Receive messages for 60 seconds
                let start = std::time::Instant::now();
                let mut message_count = 0;

                while start.elapsed() < Duration::from_secs(60) {
                    match provider.receive(&connection).await {
                        Ok(Some(item)) => match item {
                            crate::danmaku::DanmuItem::Message(msg) => {
                                println!(
                                    "[{:?}] {}: {}",
                                    msg.message_type, msg.username, msg.content
                                );
                                message_count += 1;
                            }
                            crate::danmaku::DanmuItem::Control(control) => {
                                println!("[control] {:?}", control);
                            }
                        },
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
            Err(e) => {
                println!("Failed to connect: {}", e);
            }
        }
    }
}
