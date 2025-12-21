use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};
use tracing::{debug, error, info, warn};

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::message::DanmuMessage;
use crate::danmaku::provider::{DanmuConnection, DanmuProvider};

/// Protocol definitions for a specific platform.
#[async_trait]
pub trait DanmuProtocol: Send + Sync + 'static {
    /// Platform name (e.g., "huya", "bilibili")
    fn platform(&self) -> &str;

    /// Check if the URL is supported
    fn supports_url(&self, url: &str) -> bool;

    /// Extract room ID from URL
    fn extract_room_id(&self, url: &str) -> Option<String>;

    /// Get the WebSocket URL for the room
    async fn websocket_url(&self, room_id: &str) -> Result<String>;

    /// Get custom headers for the WebSocket connection
    /// Returns a list of (header_name, header_value) pairs
    fn headers(&self, _room_id: &str) -> Vec<(String, String)> {
        vec![]
    }

    /// Get cookies for the WebSocket connection
    /// Returns an optional cookie string to be added to the Cookie header
    fn cookies(&self) -> Option<String> {
        None
    }

    /// Generate handshake messages to send upon connection
    async fn handshake_messages(&self, _room_id: &str) -> Result<Vec<Message>> {
        Ok(vec![])
    }

    /// Generate heartbeat message (if any)
    fn heartbeat_message(&self) -> Option<Message> {
        None
    }

    /// Heartbeat interval (default: 30 seconds)
    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(30)
    }

    /// Decode a WebSocket message into a list of DanmuMessages
    /// `room_id` is provided so protocols can use it for responses that need the room context
    async fn decode_message(
        &self,
        message: &Message,
        room_id: &str,
        tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuMessage>>;
}

/// Internal state for a WebSocket connection
struct WsConnectionState {
    /// Connection ID
    id: String,
    /// Room ID
    room_id: String,
    /// Connected status
    is_connected: Arc<AtomicBool>,
    /// Reconnect count
    reconnect_count: Arc<AtomicU32>,
    /// Message receiver
    message_rx: mpsc::Receiver<DanmuMessage>,
    /// Task handles
    _tasks: Vec<JoinHandle<()>>,
    /// Shutdown sender
    shutdown_tx: Option<mpsc::Sender<()>>,
}

/// A generic WebSocket-based Danmu Provider.
pub struct WebSocketDanmuProvider<P> {
    /// Protocol implementation
    protocol: P,
    /// Settings
    config: WebSocketProviderConfig,
    /// Active connections
    connections: RwLock<HashMap<String, Arc<Mutex<WsConnectionState>>>>,
}

#[derive(Clone, Copy)]
pub struct WebSocketProviderConfig {
    pub max_reconnect_attempts: u32,
    pub base_reconnect_delay_ms: u64,
    pub max_reconnect_delay_ms: u64,
}

impl Default for WebSocketProviderConfig {
    fn default() -> Self {
        Self {
            max_reconnect_attempts: 10,
            base_reconnect_delay_ms: 1000,
            max_reconnect_delay_ms: 60000,
        }
    }
}

impl<P: DanmuProtocol + Clone> WebSocketDanmuProvider<P> {
    pub fn with_protocol(protocol: P, config: Option<WebSocketProviderConfig>) -> Self {
        Self {
            protocol,
            config: config.unwrap_or_default(),
            connections: RwLock::new(HashMap::new()),
        }
    }

    async fn connect_internal(
        &self,
        room_id: &str,
    ) -> Result<(
        Arc<AtomicBool>,
        Arc<AtomicU32>,
        mpsc::Receiver<DanmuMessage>,
        mpsc::Sender<()>,
        Vec<JoinHandle<()>>,
    )> {
        let (message_tx, message_rx) = mpsc::channel(1000);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(100);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let is_connected = Arc::new(AtomicBool::new(true));
        let reconnect_count = Arc::new(AtomicU32::new(0));

        let protocol = self.protocol.clone();
        let config = self.config;
        let room_id_owned = room_id.to_string();
        let is_connected_clone = is_connected.clone();
        let reconnect_count_clone = reconnect_count.clone();

        // Spawn main management task
        let handle = tokio::spawn(async move {
            let mut current_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>> = None;
            let mut attempt = 0;
            let mut delay = config.base_reconnect_delay_ms;

            loop {
                // Check shutdown
                if let Ok(_) = shutdown_rx.try_recv() {
                    break;
                }

                // Connect if not connected
                if current_stream.is_none() {
                    match protocol.websocket_url(&room_id_owned).await {
                        Ok(url) => {
                            info!("Connecting to WebSocket: {}", url);

                            // Build request with custom headers
                            let mut headers = protocol.headers(&room_id_owned);
                            let cookies = protocol.cookies();

                            // Add or merge cookies into headers
                            if let Some(cookie_str) = cookies {
                                // Check if Cookie header already exists
                                let mut found = false;
                                for (name, value) in headers.iter_mut() {
                                    if name.eq_ignore_ascii_case("Cookie") {
                                        // Merge with existing cookie
                                        *value = format!("{}; {}", value, cookie_str);
                                        found = true;
                                        break;
                                    }
                                }
                                if !found {
                                    headers.push(("Cookie".to_string(), cookie_str));
                                }
                            }

                            let connect_result = if headers.is_empty() {
                                connect_async(&url).await
                            } else {
                                use tokio_tungstenite::tungstenite::handshake::client::generate_key;
                                use tokio_tungstenite::tungstenite::http::Request;

                                // Extract host from URL for Host header
                                let uri: tokio_tungstenite::tungstenite::http::Uri =
                                    url.parse().unwrap();
                                let host = uri.host().unwrap_or("localhost");
                                let port = uri.port_u16();
                                let host_header = if let Some(p) = port {
                                    format!("{}:{}", host, p)
                                } else {
                                    host.to_string()
                                };

                                let mut builder = Request::builder()
                                    .uri(&url)
                                    .header("Host", host_header)
                                    .header("Connection", "Upgrade")
                                    .header("Upgrade", "websocket")
                                    .header("Sec-WebSocket-Version", "13")
                                    .header("Sec-WebSocket-Key", generate_key());

                                for (name, value) in headers {
                                    builder = builder.header(name, value);
                                }
                                match builder.body(()) {
                                    Ok(request) => connect_async(request).await,
                                    Err(e) => {
                                        error!("Failed to build request: {}", e);
                                        continue;
                                    }
                                }
                            };

                            match connect_result {
                                Ok((mut ws_stream, _)) => {
                                    info!("Connected to WebSocket for room {}", room_id_owned);

                                    // Handshake
                                    let mut handshake_ok = true;
                                    if let Ok(msgs) =
                                        protocol.handshake_messages(&room_id_owned).await
                                    {
                                        for msg in msgs {
                                            if let Err(e) = ws_stream.send(msg).await {
                                                error!("Handshake failed: {}", e);
                                                handshake_ok = false;
                                                break;
                                            }
                                        }
                                    }

                                    if !handshake_ok {
                                        continue; // Retry connection
                                    }

                                    is_connected_clone.store(true, Ordering::SeqCst);
                                    reconnect_count_clone.store(0, Ordering::SeqCst);
                                    attempt = 0;
                                    delay = config.base_reconnect_delay_ms;
                                    current_stream = Some(ws_stream);
                                }
                                Err(e) => {
                                    warn!("Connection failed: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to get WebSocket URL: {}", e);
                        }
                    }

                    if current_stream.is_none() {
                        if attempt >= config.max_reconnect_attempts {
                            error!("Max reconnect attempts reached for {}", room_id_owned);
                            break;
                        }
                        attempt += 1;
                        reconnect_count_clone.store(attempt, Ordering::SeqCst);

                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
                            _ = shutdown_rx.recv() => break,
                        }

                        delay = (delay * 2).min(config.max_reconnect_delay_ms);
                        continue;
                    }
                }

                // Main loop: read/write/heartbeat
                if let Some(mut stream) = current_stream.take() {
                    let heartbeat_enabled = protocol.heartbeat_message().is_some();
                    let heartbeat_interval = protocol.heartbeat_interval();
                    let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
                    heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let response_tx_clone = response_tx.clone();

                    loop {
                        tokio::select! {
                             // Heartbeat (only if enabled)
                            _ = heartbeat_timer.tick(), if heartbeat_enabled => {
                                if let Some(msg) = protocol.heartbeat_message() {
                                    if let Err(e) = stream.send(msg).await {
                                        error!("Failed to send heartbeat: {}", e);
                                        break; // Reconnect
                                    }
                                    debug!("Sent heartbeat for {}", room_id_owned);
                                }
                            }

                            // Response messages from protocol
                            Some(msg) = response_rx.recv() => {
                                if let Err(e) = stream.send(msg).await {
                                    error!("Failed to send response message: {}", e);
                                    break; // Reconnect
                                }
                            }

                            // Read message
                            msg_opt = stream.next() => {
                                match msg_opt {
                                    Some(Ok(msg)) => {
                                        match protocol.decode_message(&msg, &room_id_owned, &response_tx_clone).await {
                                            Ok(messages) => {
                                                for danmu in messages {
                                                    if message_tx.send(danmu).await.is_err() {
                                                        break; // Channel closed
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!("Failed to decode message: {}", e);
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        error!("WebSocket error: {}", e);
                                        break; // Reconnect
                                    }
                                    None => {
                                        warn!("WebSocket stream closed");
                                        break; // Reconnect
                                    }
                                }
                            }

                            // Shutdown
                            _ = shutdown_rx.recv() => {
                                let _ = stream.close(None).await;
                                return;
                            }
                        }
                    }

                    // If we break internal loop, connection is lost/broken
                    is_connected_clone.store(false, Ordering::SeqCst);
                }
            }
            debug!("WebSocket task for {} stopped", room_id_owned);
        });

        Ok((
            is_connected,
            reconnect_count,
            message_rx,
            shutdown_tx,
            vec![handle],
        ))
    }
}

#[async_trait]
impl<P: DanmuProtocol + Clone> DanmuProvider for WebSocketDanmuProvider<P> {
    fn platform(&self) -> &str {
        self.protocol.platform()
    }

    async fn connect(&self, room_id: &str) -> Result<DanmuConnection> {
        let connection_id = format!("{}-{}-{}", self.platform(), room_id, uuid::Uuid::new_v4());

        let (is_connected, reconnect_count, message_rx, shutdown_tx, tasks) =
            self.connect_internal(room_id).await?;

        let state = WsConnectionState {
            id: connection_id.clone(),
            room_id: room_id.to_string(),
            is_connected,
            reconnect_count,
            message_rx,
            _tasks: tasks,
            shutdown_tx: Some(shutdown_tx),
        };

        self.connections
            .write()
            .await
            .insert(connection_id.clone(), Arc::new(Mutex::new(state)));

        let mut conn = DanmuConnection::new(connection_id, self.platform(), room_id);
        // It might take a moment to actually connect, but we return the handle immediately.
        // The service will check `receive` which handles the logic.
        conn.set_connected();

        Ok(conn)
    }

    async fn disconnect(&self, connection: &mut DanmuConnection) -> Result<()> {
        if let Some(state_arc) = self.connections.write().await.remove(&connection.id) {
            let mut state = state_arc.lock().await;
            if let Some(tx) = state.shutdown_tx.take() {
                let _ = tx.send(()).await;
            }
        }
        connection.set_disconnected();
        Ok(())
    }

    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuMessage>> {
        let map = self.connections.read().await;
        if let Some(state_arc) = map.get(&connection.id) {
            let mut state = state_arc.lock().await;

            // Simple check
            match tokio::time::timeout(Duration::from_millis(100), state.message_rx.recv()).await {
                Ok(Some(msg)) => Ok(Some(msg)),
                Ok(None) => Err(DanmakuError::connection("Channel closed")),
                Err(_) => Ok(None), // Timeout
            }
        } else {
            Err(DanmakuError::connection("Connection not found"))
        }
    }

    fn supports_url(&self, url: &str) -> bool {
        self.protocol.supports_url(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        self.protocol.extract_room_id(url)
    }
}
