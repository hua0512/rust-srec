use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};
use tracing::{debug, error, info, trace, warn};

use crate::danmaku::ConnectionConfig;
use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::event::DanmuItem;
use crate::danmaku::provider::{DanmuConnection, DanmuProvider};

const MAX_ACTIVE_CONNECTIONS: usize = 1024;

fn parse_cookie_header(input: &str) -> Vec<(String, String)> {
    input
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }

            let mut kv = part.splitn(2, '=');
            let name = kv.next()?.trim();
            let value = kv.next()?.trim();
            if name.is_empty() || value.is_empty() {
                return None;
            }

            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn merge_cookie_headers(base: Option<&str>, extra: Option<&str>) -> Option<String> {
    let base = base.map(str::trim).filter(|s| !s.is_empty());
    let extra = extra.map(str::trim).filter(|s| !s.is_empty());

    match (base, extra) {
        (None, None) => None,
        (Some(base), None) => Some(base.to_string()),
        (None, Some(extra)) => Some(extra.to_string()),
        (Some(base), Some(extra)) => {
            let mut parts = parse_cookie_header(base);
            let mut index_by_name: HashMap<String, usize> = HashMap::with_capacity(parts.len());
            for (idx, (name, _)) in parts.iter().enumerate() {
                index_by_name.insert(name.clone(), idx);
            }

            for (name, value) in parse_cookie_header(extra) {
                if let Some(existing_idx) = index_by_name.get(&name) {
                    parts[*existing_idx].1 = value;
                } else {
                    let idx = parts.len();
                    parts.push((name.clone(), value));
                    index_by_name.insert(name, idx);
                }
            }

            Some(
                parts
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }
}

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

    /// Get cookies for the danmu connection.
    ///
    /// When `send_cookie_header()` is true, these cookies may be sent as the `Cookie`
    /// header during the WebSocket upgrade.
    fn cookies(&self) -> Option<String> {
        None
    }

    /// Whether to send the `Cookie` header during the WebSocket upgrade.
    ///
    /// Defaults to `true` to preserve historical behavior, but platforms that don't
    /// require cookies (or where cookies are sensitive) should override to `false`.
    fn send_cookie_header(&self) -> bool {
        true
    }

    /// Normalize/adjust cookies before sending them.
    ///
    /// This is useful for platforms that require specific cookie keys to exist
    /// or need to alias cookie names.
    fn normalize_cookies(&self, cookies: &str) -> String {
        cookies.to_string()
    }

    /// Configure protocol state based on connection inputs.
    ///
    /// This is useful for passing derived session information (e.g. uid) into
    /// handshake/auth messages.
    fn configure_connection(
        &mut self,
        _cookies: Option<&str>,
        _extras: Option<&HashMap<String, String>>,
    ) {
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

    /// Decode a WebSocket message into a list of danmu items (messages or control events).
    /// `room_id` is provided so protocols can use it for responses that need the room context
    async fn decode_message(
        &self,
        message: &Message,
        room_id: &str,
        tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuItem>>;
}

/// Internal state for a WebSocket connection
struct WsConnectionState {
    /// Connection ID
    #[allow(dead_code)]
    id: String,
    /// Room ID
    #[allow(dead_code)]
    room_id: String,
    /// Connected status
    #[allow(dead_code)]
    is_connected: Arc<AtomicBool>,
    /// Reconnect count
    #[allow(dead_code)]
    reconnect_count: Arc<AtomicU32>,
    /// Message receiver
    message_rx: mpsc::Receiver<DanmuItem>,
    /// Task handles
    tasks: Vec<JoinHandle<()>>,
    /// Shutdown sender
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Limits total active connections to prevent unbounded growth if callers forget to disconnect.
    #[allow(dead_code)]
    connection_permit: OwnedSemaphorePermit,
}

impl WsConnectionState {
    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for WsConnectionState {
    fn drop(&mut self) {
        self.abort_tasks();
    }
}

/// A generic WebSocket-based Danmu Provider.
pub struct WebSocketDanmuProvider<P> {
    /// Protocol implementation
    protocol: P,
    /// Settings
    config: WebSocketProviderConfig,
    /// Active connections
    connections: RwLock<HashMap<String, Arc<Mutex<WsConnectionState>>>>,
    connection_semaphore: Arc<Semaphore>,
}

#[derive(Clone, Copy, Debug)]
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
            connection_semaphore: Arc::new(Semaphore::new(MAX_ACTIVE_CONNECTIONS)),
        }
    }

    async fn connect_internal(
        &self,
        room_id: &str,
        config: ConnectionConfig,
    ) -> Result<(
        Arc<AtomicBool>,
        Arc<AtomicU32>,
        mpsc::Receiver<DanmuItem>,
        mpsc::Sender<()>,
        Vec<JoinHandle<()>>,
    )> {
        let is_connected = Arc::new(AtomicBool::new(false));
        let reconnect_count = Arc::new(AtomicU32::new(0));
        let (message_tx, message_rx) = mpsc::channel(100);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(100);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        let mut protocol = self.protocol.clone();
        let ws_config = config.websocket.unwrap_or(self.config);
        let room_id_owned = room_id.to_string();
        let cookies = config.cookies;
        let extras = config.extras;
        let is_connected_clone = is_connected.clone();
        let reconnect_count_clone = reconnect_count.clone();

        // Spawn main management task
        let handle = tokio::spawn(async move {
            let mut current_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>> = None;
            let mut attempt = 0;
            let mut delay = ws_config.base_reconnect_delay_ms;

            loop {
                // Check shutdown
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }

                // Connect if not connected
                if current_stream.is_none() {
                    // Compute cookies and let the protocol derive per-connection state (e.g. uid)
                    // before resolving the final WebSocket URL.
                    let protocol_cookies = protocol.cookies();
                    let merged_cookies =
                        merge_cookie_headers(protocol_cookies.as_deref(), cookies.as_deref())
                            .map(|c| protocol.normalize_cookies(&c));
                    protocol.configure_connection(merged_cookies.as_deref(), extras.as_ref());

                    match protocol.websocket_url(&room_id_owned).await {
                        Ok(url) => {
                            info!("Connecting to WebSocket: {}", url);

                            // Build request with custom headers
                            let mut headers = protocol.headers(&room_id_owned);

                            if protocol.send_cookie_header() {
                                // Add or merge cookies into headers
                                if let Some(cookie_str) = merged_cookies.clone() {
                                    // Check if Cookie header already exists
                                    let mut found = false;
                                    for (name, value) in headers.iter_mut() {
                                        if name.eq_ignore_ascii_case("Cookie") {
                                            // Merge with existing cookie
                                            let merged = merge_cookie_headers(
                                                Some(value.as_str()),
                                                Some(cookie_str.as_str()),
                                            )
                                            .unwrap_or_else(|| cookie_str.clone());
                                            *value = protocol.normalize_cookies(&merged);
                                            found = true;
                                            break;
                                        }
                                    }
                                    if !found {
                                        headers.push(("Cookie".to_string(), cookie_str));
                                    }
                                }
                            } else {
                                // Explicitly drop any Cookie header to avoid leakage.
                                headers.retain(|(name, _)| !name.eq_ignore_ascii_case("Cookie"));
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
                                    delay = ws_config.base_reconnect_delay_ms;
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
                        if attempt >= ws_config.max_reconnect_attempts {
                            error!("Max reconnect attempts reached for {}", room_id_owned);
                            break;
                        }
                        attempt += 1;
                        reconnect_count_clone.store(attempt, Ordering::SeqCst);

                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
                            _ = shutdown_rx.recv() => break,
                        }

                        delay = (delay * 2).min(ws_config.max_reconnect_delay_ms);
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
                                    trace!("Sent heartbeat for {}", room_id_owned);
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
                                                for item in messages {
                                                    if message_tx.send(item).await.is_err() {
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

    async fn connect(&self, room_id: &str, config: ConnectionConfig) -> Result<DanmuConnection> {
        let connection_permit = self
            .connection_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                DanmakuError::connection(format!(
                    "Too many active connections (max {})",
                    MAX_ACTIVE_CONNECTIONS
                ))
            })?;

        let connection_id = format!("{}-{}-{}", self.platform(), room_id, uuid::Uuid::new_v4());

        let (is_connected, reconnect_count, message_rx, shutdown_tx, tasks) =
            self.connect_internal(room_id, config).await?;

        let state = WsConnectionState {
            id: connection_id.clone(),
            room_id: room_id.to_string(),
            is_connected,
            reconnect_count,
            message_rx,
            tasks,
            shutdown_tx: Some(shutdown_tx),
            connection_permit,
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
                let _ = tx.try_send(());
            }
            state.abort_tasks();
        }
        connection.set_disconnected();
        Ok(())
    }

    async fn receive(&self, connection: &DanmuConnection) -> Result<Option<DanmuItem>> {
        let state_arc = {
            let map = self.connections.read().await;
            map.get(&connection.id).cloned()
        };

        let Some(state_arc) = state_arc else {
            return Err(DanmakuError::connection("Connection not found"));
        };

        let mut state = state_arc.lock().await;
        match tokio::time::timeout(Duration::from_millis(100), state.message_rx.recv()).await {
            Ok(Some(msg)) => Ok(Some(msg)),
            Ok(None) => {
                drop(state);
                let _ = self.connections.write().await.remove(&connection.id);
                Err(DanmakuError::connection("Channel closed"))
            }
            Err(_) => Ok(None), // Timeout
        }
    }

    fn supports_url(&self, url: &str) -> bool {
        self.protocol.supports_url(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        self.protocol.extract_room_id(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_cookie_headers_merges_and_overrides() {
        let base = "a=1; b=2";
        let extra = "b=3; c=4";
        assert_eq!(
            merge_cookie_headers(Some(base), Some(extra)).as_deref(),
            Some("a=1; b=3; c=4")
        );
    }

    #[test]
    fn test_merge_cookie_headers_ignores_empty_parts() {
        let base = "a=1; ; b=2; invalid";
        let extra = "b=3; c=4; =nope; d=";
        assert_eq!(
            merge_cookie_headers(Some(base), Some(extra)).as_deref(),
            Some("a=1; b=3; c=4")
        );
    }
}
