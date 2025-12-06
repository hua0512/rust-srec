//! Twitch danmu (chat) provider.
//!
//! Implements danmu collection for the Twitch streaming platform using IRC.

use async_trait::async_trait;
use regex::Regex;
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::danmu::{DanmuConnection, DanmuMessage, DanmuProvider, DanmuType};
use crate::error::{Error, Result};

/// Twitch IRC server address (TLS)
const TWITCH_IRC_HOST: &str = "irc.chat.twitch.tv";
/// Twitch IRC TLS port
const TWITCH_IRC_TLS_PORT: u16 = 6697;

/// Maximum reconnection attempts
const MAX_RECONNECT_ATTEMPTS: u32 = 10;
/// Base delay for exponential backoff (in milliseconds)
const BASE_RECONNECT_DELAY_MS: u64 = 1000;
/// Maximum delay for exponential backoff (in milliseconds)
const MAX_RECONNECT_DELAY_MS: u64 = 60000;

/// Twitch IRC connection state
struct TwitchConnectionState {
    /// Channel name
    channel: String,
    /// Whether the connection is active
    is_connected: Arc<AtomicBool>,
    /// Reconnection count
    reconnect_count: Arc<AtomicU32>,
    /// Message receiver channel
    message_rx: mpsc::Receiver<DanmuMessage>,
    /// Message processing task handle
    message_handle: Option<JoinHandle<()>>,
    /// Reconnection task handle
    reconnect_handle: Option<JoinHandle<()>>,
    /// Shutdown signal sender
    shutdown_tx: Option<mpsc::Sender<()>>,
}

/// Shared state between connection and tasks
struct SharedConnectionState {
    /// Whether the connection is active
    is_connected: Arc<AtomicBool>,
    /// Reconnection count
    reconnect_count: Arc<AtomicU32>,
    /// Message sender channel
    message_tx: mpsc::Sender<DanmuMessage>,
    /// Shutdown signal receiver
    shutdown_rx: mpsc::Receiver<()>,
    /// Shutdown signal sender (for cloning)
    shutdown_tx: mpsc::Sender<()>,
}

/// Twitch danmu provider using IRC protocol.
pub struct TwitchDanmuProvider {
    /// Regex for extracting channel name from URL
    url_regex: OnceLock<Regex>,
    /// Active connections (connection_id -> state)
    connections: RwLock<FxHashMap<String, Arc<Mutex<TwitchConnectionState>>>>,
}

impl TwitchDanmuProvider {
    /// Create a new Twitch danmu provider.
    pub fn new() -> Self {
        Self {
            url_regex: OnceLock::new(),
            connections: RwLock::new(FxHashMap::default()),
        }
    }

    fn get_url_regex(&self) -> &Regex {
        self.url_regex.get_or_init(|| {
            Regex::new(r"(?:https?://)?(?:www\.)?twitch\.tv/([a-zA-Z0-9_]+)").unwrap()
        })
    }

    /// Connect to Twitch IRC server with TLS
    async fn connect_irc(
        &self,
        channel: &str,
    ) -> Result<(
        tokio_rustls::client::TlsStream<TcpStream>,
        SharedConnectionState,
        mpsc::Receiver<DanmuMessage>,
    )> {
        info!(
            "Connecting to Twitch IRC server: {}:{}",
            TWITCH_IRC_HOST, TWITCH_IRC_TLS_PORT
        );

        // Create TCP connection
        let tcp_stream = TcpStream::connect((TWITCH_IRC_HOST, TWITCH_IRC_TLS_PORT))
            .await
            .map_err(|e| Error::DanmuError(format!("Failed to connect to Twitch IRC: {}", e)))?;

        // Set up TLS
        let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(TWITCH_IRC_HOST)
            .map_err(|e| Error::DanmuError(format!("Invalid server name: {}", e)))?;

        let tls_stream = connector
            .connect(server_name.to_owned(), tcp_stream)
            .await
            .map_err(|e| Error::DanmuError(format!("TLS handshake failed: {}", e)))?;

        info!(
            "Connected to Twitch IRC server with TLS for channel #{}",
            channel
        );

        let (message_tx, message_rx) = mpsc::channel(1000);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let is_connected = Arc::new(AtomicBool::new(true));
        let reconnect_count = Arc::new(AtomicU32::new(0));

        let shared_state = SharedConnectionState {
            is_connected,
            reconnect_count,
            message_tx,
            shutdown_rx,
            shutdown_tx,
        };

        Ok((tls_stream, shared_state, message_rx))
    }

    /// Send IRC command to the server
    async fn send_command(
        stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
        command: &str,
    ) -> Result<()> {
        let cmd = format!("{}\r\n", command);
        stream
            .write_all(cmd.as_bytes())
            .await
            .map_err(|e| Error::DanmuError(format!("Failed to send IRC command: {}", e)))?;
        stream
            .flush()
            .await
            .map_err(|e| Error::DanmuError(format!("Failed to flush IRC stream: {}", e)))?;
        debug!("Sent IRC command: {}", command);
        Ok(())
    }

    /// Authenticate with Twitch IRC (anonymous)
    async fn authenticate(stream: &mut tokio_rustls::client::TlsStream<TcpStream>) -> Result<()> {
        // Generate random anonymous username
        let random_num: u32 = rand::random::<u32>() % 100000;
        let nick = format!("justinfan{}", random_num);

        // Send PASS (empty for anonymous)
        Self::send_command(stream, "PASS oauth:").await?;

        // Send NICK
        Self::send_command(stream, &format!("NICK {}", nick)).await?;

        // Request Twitch capabilities for tags and commands
        Self::send_command(stream, "CAP REQ :twitch.tv/tags twitch.tv/commands").await?;

        info!("Authenticated with Twitch IRC as {}", nick);
        Ok(())
    }

    /// Join a channel
    async fn join_channel(
        stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
        channel: &str,
    ) -> Result<()> {
        let channel_name = if channel.starts_with('#') {
            channel.to_string()
        } else {
            format!("#{}", channel.to_lowercase())
        };

        Self::send_command(stream, &format!("JOIN {}", channel_name)).await?;
        info!("Joined Twitch channel {}", channel_name);
        Ok(())
    }

    /// Calculate reconnection delay with exponential backoff
    fn calculate_reconnect_delay(attempt: u32) -> Duration {
        let delay_ms = BASE_RECONNECT_DELAY_MS * 2u64.pow(attempt.min(10));
        Duration::from_millis(delay_ms.min(MAX_RECONNECT_DELAY_MS))
    }

    /// Attempt to reconnect to the IRC server with exponential backoff
    async fn attempt_reconnect(
        channel: &str,
        reconnect_count: &Arc<AtomicU32>,
        is_connected: &Arc<AtomicBool>,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let mut attempt = 0;

        while attempt < MAX_RECONNECT_ATTEMPTS {
            let current_count = reconnect_count.fetch_add(1, Ordering::SeqCst);
            let delay = Self::calculate_reconnect_delay(current_count);

            warn!(
                "Attempting to reconnect to Twitch IRC (attempt {}/{}), waiting {:?}",
                attempt + 1,
                MAX_RECONNECT_ATTEMPTS,
                delay
            );

            tokio::time::sleep(delay).await;

            // Try to establish new connection
            match TcpStream::connect((TWITCH_IRC_HOST, TWITCH_IRC_TLS_PORT)).await {
                Ok(tcp_stream) => {
                    // Set up TLS
                    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
                    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

                    let config = tokio_rustls::rustls::ClientConfig::builder()
                        .with_root_certificates(root_store)
                        .with_no_client_auth();

                    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
                    let server_name =
                        tokio_rustls::rustls::pki_types::ServerName::try_from(TWITCH_IRC_HOST)
                            .map_err(|e| {
                                Error::DanmuError(format!("Invalid server name: {}", e))
                            })?;

                    match connector.connect(server_name.to_owned(), tcp_stream).await {
                        Ok(mut tls_stream) => {
                            // Re-authenticate and join channel
                            if let Err(e) = Self::authenticate(&mut tls_stream).await {
                                error!("Failed to authenticate after reconnect: {}", e);
                                attempt += 1;
                                continue;
                            }

                            if let Err(e) = Self::join_channel(&mut tls_stream, channel).await {
                                error!("Failed to join channel after reconnect: {}", e);
                                attempt += 1;
                                continue;
                            }

                            // Reset reconnect count on successful connection
                            reconnect_count.store(0, Ordering::SeqCst);
                            is_connected.store(true, Ordering::SeqCst);
                            info!(
                                "Successfully reconnected to Twitch IRC for channel #{}",
                                channel
                            );
                            return Ok(tls_stream);
                        }
                        Err(e) => {
                            error!("TLS handshake failed during reconnect: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect during reconnect: {}", e);
                }
            }

            attempt += 1;
        }

        Err(Error::DanmuError(format!(
            "Failed to reconnect after {} attempts",
            MAX_RECONNECT_ATTEMPTS
        )))
    }

    /// Start the message processing task with reconnection support
    fn start_message_task(
        read_half: tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
        write_half: Arc<Mutex<tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>>>,
        message_tx: mpsc::Sender<DanmuMessage>,
        is_connected: Arc<AtomicBool>,
        reconnect_count: Arc<AtomicU32>,
        channel: String,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let reader = BufReader::new(read_half);
            let mut line = String::new();
            let mut current_write_half = write_half;
            let mut current_reader = reader;
            let mut shutdown_rx = shutdown_rx;

            loop {
                // Check for shutdown signal
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Received shutdown signal, stopping message task");
                        break;
                    }
                    result = current_reader.read_line(&mut line) => {
                        match result {
                            Ok(0) => {
                                // Connection closed - attempt reconnection
                                warn!("Twitch IRC connection closed, attempting reconnection");
                                is_connected.store(false, Ordering::SeqCst);

                                match Self::attempt_reconnect(
                                    &channel,
                                    &reconnect_count,
                                    &is_connected,
                                ).await {
                                    Ok(new_stream) => {
                                        let (new_read, new_write) = tokio::io::split(new_stream);
                                        current_reader = BufReader::new(new_read);
                                        current_write_half = Arc::new(Mutex::new(new_write));
                                        line.clear();
                                        continue;
                                    }
                                    Err(e) => {
                                        error!("Failed to reconnect: {}", e);
                                        break;
                                    }
                                }
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    line.clear();
                                    continue;
                                }

                                debug!("Received IRC line: {}", trimmed);

                                // Handle PING
                                if trimmed.starts_with("PING") {
                                    let ping_data = trimmed.strip_prefix("PING ").unwrap_or(":tmi.twitch.tv");
                                    let mut writer = current_write_half.lock().await;
                                    let pong = format!("PONG {}\r\n", ping_data);
                                    if let Err(e) = writer.write_all(pong.as_bytes()).await {
                                        error!("Failed to send PONG: {}", e);
                                        // Connection might be broken, mark as disconnected
                                        is_connected.store(false, Ordering::SeqCst);
                                    }
                                    let _ = writer.flush().await;
                                    debug!("Responded to PING");
                                    line.clear();
                                    continue;
                                }

                                // Parse IRC message
                                match parse_twitch_irc_message(trimmed) {
                                    Ok(Some(msg)) => {
                                        if message_tx.send(msg).await.is_err() {
                                            warn!("Message channel closed");
                                            break;
                                        }
                                    }
                                    Ok(None) => {
                                        // Not a chat message, ignore
                                    }
                                    Err(e) => {
                                        debug!("Failed to parse IRC message: {}", e);
                                    }
                                }
                                line.clear();
                            }
                            Err(e) => {
                                error!("Error reading from IRC stream: {}", e);
                                is_connected.store(false, Ordering::SeqCst);

                                // Attempt reconnection
                                match Self::attempt_reconnect(
                                    &channel,
                                    &reconnect_count,
                                    &is_connected,
                                ).await {
                                    Ok(new_stream) => {
                                        let (new_read, new_write) = tokio::io::split(new_stream);
                                        current_reader = BufReader::new(new_read);
                                        current_write_half = Arc::new(Mutex::new(new_write));
                                        line.clear();
                                        continue;
                                    }
                                    Err(e) => {
                                        error!("Failed to reconnect after error: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            debug!("Message processing task stopped");
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
        let connection_id = format!("twitch-{}-{}", room_id, uuid::Uuid::new_v4());

        // Establish IRC connection
        let (mut tls_stream, shared_state, message_rx) = self.connect_irc(room_id).await?;

        // Authenticate and join channel
        Self::authenticate(&mut tls_stream).await?;
        Self::join_channel(&mut tls_stream, room_id).await?;

        // Split the stream for reading and writing
        let (read_half, write_half) = tokio::io::split(tls_stream);
        let write_half = Arc::new(Mutex::new(write_half));

        // Start message processing task with reconnection support
        let message_handle = Self::start_message_task(
            read_half,
            write_half,
            shared_state.message_tx.clone(),
            shared_state.is_connected.clone(),
            shared_state.reconnect_count.clone(),
            room_id.to_string(),
            shared_state.shutdown_rx,
        );

        // Create connection
        let mut connection = DanmuConnection::new(connection_id.clone(), "twitch", room_id);
        connection.set_connected();

        // Store connection state
        let state = TwitchConnectionState {
            channel: room_id.to_string(),
            is_connected: shared_state.is_connected,
            reconnect_count: shared_state.reconnect_count,
            message_rx,
            message_handle: Some(message_handle),
            reconnect_handle: None,
            shutdown_tx: Some(shared_state.shutdown_tx),
        };

        self.connections
            .write()
            .await
            .insert(connection_id, Arc::new(Mutex::new(state)));

        Ok(connection)
    }

    /// Disconnect from Twitch IRC.
    ///
    /// # Cancel Safety
    ///
    /// This method uses graceful shutdown with timeout. If cancelled:
    /// - The shutdown signal may or may not have been sent
    /// - Tasks may continue running until their next check point
    /// - Connection state will be marked as disconnected on next call
    async fn disconnect(&self, connection: &mut DanmuConnection) -> Result<()> {
        /// Timeout for graceful shutdown of tasks
        const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
        /// Shorter timeout for reconnect task
        const RECONNECT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

        if let Some(state) = self.connections.write().await.remove(&connection.id) {
            let mut state = state.lock().await;

            // Step 1: Signal graceful shutdown via existing shutdown_tx channel
            if let Some(shutdown_tx) = state.shutdown_tx.take() {
                let _ = shutdown_tx.send(()).await;
            }

            state.is_connected.store(false, Ordering::SeqCst);

            // Step 2: Wait for message task to complete gracefully with timeout
            if let Some(handle) = state.message_handle.take() {
                match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, handle).await {
                    Ok(Ok(())) => {
                        debug!("Message task stopped gracefully");
                    }
                    Ok(Err(e)) => {
                        // Task panicked or was cancelled
                        debug!("Message task ended with error: {}", e);
                    }
                    Err(_) => {
                        // Timeout - task didn't respond to shutdown signal
                        warn!(
                            "Message task did not stop within {:?}, task will be dropped",
                            GRACEFUL_SHUTDOWN_TIMEOUT
                        );
                        // Note: The JoinHandle is dropped here, which cancels the task
                    }
                }
            }

            // Step 3: Wait for reconnect task to complete gracefully with timeout
            if let Some(handle) = state.reconnect_handle.take() {
                match tokio::time::timeout(RECONNECT_SHUTDOWN_TIMEOUT, handle).await {
                    Ok(Ok(())) => {
                        debug!("Reconnect task stopped gracefully");
                    }
                    Ok(Err(e)) => {
                        debug!("Reconnect task ended with error: {}", e);
                    }
                    Err(_) => {
                        warn!(
                            "Reconnect task did not stop within {:?}, task will be dropped",
                            RECONNECT_SHUTDOWN_TIMEOUT
                        );
                    }
                }
            }

            info!("Disconnected from Twitch IRC channel #{}", state.channel);
        }

        connection.set_disconnected();
        Ok(())
    }

    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuMessage>> {
        if !connection.is_connected {
            return Err(Error::DanmuError("Connection is not active".to_string()));
        }

        let connections = self.connections.read().await;
        if let Some(state) = connections.get(&connection.id) {
            let mut state = state.lock().await;

            // Check if still connected
            if !state.is_connected.load(Ordering::SeqCst) {
                return Err(Error::DanmuError("Connection lost".to_string()));
            }

            // Try to receive a message with timeout
            match tokio::time::timeout(Duration::from_millis(100), state.message_rx.recv()).await {
                Ok(Some(msg)) => Ok(Some(msg)),
                Ok(None) => {
                    // Channel closed
                    Err(Error::DanmuError("Message channel closed".to_string()))
                }
                Err(_) => {
                    // Timeout - no message available
                    Ok(None)
                }
            }
        } else {
            Err(Error::DanmuError("Connection not found".to_string()))
        }
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
///
/// Twitch IRC format with tags:
/// @badge-info=;badges=;color=#FF0000;display-name=User;emotes=;id=xxx;mod=0;room-id=123;
/// subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=456;user-type=
/// :user!user@user.tmi.twitch.tv PRIVMSG #channel :message content
pub fn parse_twitch_irc_message(line: &str) -> Result<Option<DanmuMessage>> {
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
        assert_eq!(provider.extract_room_id("https://www.huya.com/12345"), None);
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

    #[test]
    fn test_parse_bits_message() {
        let line = "@badge-info=;badges=bits/100;bits=100;color=#FF0000;display-name=Cheerer;emotes=;id=abc123;mod=0;room-id=12345;subscriber=0;tmi-sent-ts=1234567890;turbo=0;user-id=67890;user-type= :cheerer!cheerer@cheerer.tmi.twitch.tv PRIVMSG #channel :cheer100 Great stream!";

        let result = parse_twitch_irc_message(line).unwrap();
        assert!(result.is_some());

        let msg = result.unwrap();
        assert_eq!(msg.message_type, DanmuType::Gift);
        assert!(msg.metadata.is_some());
        let metadata = msg.metadata.unwrap();
        assert_eq!(metadata.get("bits").unwrap(), &serde_json::json!(100));
    }

    #[test]
    fn test_calculate_reconnect_delay() {
        // First attempt: 1000ms
        assert_eq!(
            TwitchDanmuProvider::calculate_reconnect_delay(0),
            Duration::from_millis(1000)
        );
        // Second attempt: 2000ms
        assert_eq!(
            TwitchDanmuProvider::calculate_reconnect_delay(1),
            Duration::from_millis(2000)
        );
        // Third attempt: 4000ms
        assert_eq!(
            TwitchDanmuProvider::calculate_reconnect_delay(2),
            Duration::from_millis(4000)
        );
        // Max delay should be capped
        assert_eq!(
            TwitchDanmuProvider::calculate_reconnect_delay(20),
            Duration::from_millis(MAX_RECONNECT_DELAY_MS)
        );
    }
}
