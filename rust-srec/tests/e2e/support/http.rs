//! Loopback stand-ins for the hosts the backend calls out to.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A loopback address nothing listens on, so connections to it are refused.
///
/// The port stays bound, without listening, for the value's lifetime, so no
/// other test can take it over meanwhile. Except on macOS: BSD stacks silently
/// drop a SYN aimed at a bound but non-listening socket, so the connection would
/// time out instead of being refused; there the port is released at once.
pub struct ClosedPort {
    address: SocketAddr,
    _socket: Option<tokio::net::TcpSocket>,
}

impl ClosedPort {
    pub fn reserve() -> Self {
        let socket = tokio::net::TcpSocket::new_v4().expect("create a socket");
        socket
            .bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("bind a loopback port");
        let address = socket.local_addr().expect("the bound address");
        Self {
            address,
            _socket: (!cfg!(target_os = "macos")).then_some(socket),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }
}

/// A request received by [`LocalHttpServer`].
#[derive(Debug, Clone)]
pub struct ReceivedRequest {
    pub method: String,
    /// The request target as sent: a path and query, or an absolute URL for
    /// a forward-proxy request.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ReceivedRequest {
    /// A header's value, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub(super) async fn from_axum(request: axum::extract::Request) -> Self {
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, 16 * 1024 * 1024)
            .await
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        Self {
            method: parts.method.to_string(),
            path: parts.uri.to_string(),
            headers,
            body,
        }
    }
}

/// A loopback HTTP server that records every request. Use it as the outbound
/// target for webhooks and similar integrations so tests never leave the
/// machine.
pub struct LocalHttpServer {
    address: SocketAddr,
    received: Arc<Mutex<Vec<ReceivedRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl LocalHttpServer {
    /// Answer every request with `status` and `reply`.
    pub async fn start(status: u16, reply: &'static str) -> Self {
        use axum::response::IntoResponse;

        let status = axum::http::StatusCode::from_u16(status).expect("valid status");
        Self::start_with(move |_, _| (status, reply).into_response()).await
    }

    /// Answer each request with `reply(address, request)`, where `address`
    /// is this server's own, for replies that link back to it.
    pub async fn start_with<F>(reply: F) -> Self
    where
        F: Fn(SocketAddr, &ReceivedRequest) -> axum::response::Response + Send + Sync + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the local HTTP server");
        let address = listener.local_addr().expect("local HTTP server address");
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorder = received.clone();
        let reply = Arc::new(reply);
        let handler = move |request: axum::extract::Request| {
            let recorder = recorder.clone();
            let reply = reply.clone();
            async move {
                let request = ReceivedRequest::from_axum(request).await;
                let response = reply(address, &request);
                recorder.lock().expect("request log lock").push(request);
                response
            }
        };
        let app = axum::Router::new().fallback(handler);
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve the local HTTP server");
        });
        Self {
            address,
            received,
            task,
        }
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    /// Requests received so far.
    pub fn received(&self) -> Vec<ReceivedRequest> {
        self.received.lock().expect("request log lock").clone()
    }
}

impl Drop for LocalHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A request line a [`ConnectRecorder`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyAttempt {
    pub method: String,
    /// `host:port` for a CONNECT, the absolute URL for a forwarded request.
    pub target: String,
}

/// An HTTP proxy that answers every request with 502 and records its request
/// line. Used as the route of everything that must not leave the machine: an
/// HTTP proxy client resolves no host name itself, so each attempt shows up
/// here instead of on the network.
pub struct ConnectRecorder {
    address: SocketAddr,
    attempts: Arc<Mutex<Vec<ProxyAttempt>>>,
    task: tokio::task::JoinHandle<()>,
}

/// Longest request head the recorder reads before answering.
const MAX_PROXY_REQUEST_HEAD: usize = 64 * 1024;

impl ConnectRecorder {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy recorder");
        let address = listener.local_addr().expect("proxy recorder address");
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let recorded = attempts.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        connections.spawn(refuse(stream, recorded.clone()));
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self {
            address,
            attempts,
            task,
        }
    }

    /// The proxy URL, as a saved proxy stores it.
    pub fn proxy_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Request lines refused so far.
    pub fn attempts(&self) -> Vec<ProxyAttempt> {
        self.attempts
            .lock()
            .expect("proxy attempt log lock")
            .clone()
    }
}

async fn refuse(mut stream: tokio::net::TcpStream, attempts: Arc<Mutex<Vec<ProxyAttempt>>>) {
    let mut head = Vec::new();
    let mut buffer = [0u8; 4096];
    let read_head = async {
        while !head.windows(4).any(|window| window == b"\r\n\r\n")
            && head.len() < MAX_PROXY_REQUEST_HEAD
        {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(5), read_head)
        .await
        .is_err()
    {
        return;
    }
    let head = String::from_utf8_lossy(&head);
    let mut request_line = head.lines().next().unwrap_or_default().split(' ');
    if let (Some(method), Some(target)) = (request_line.next(), request_line.next()) {
        attempts
            .lock()
            .expect("proxy attempt log lock")
            .push(ProxyAttempt {
                method: method.to_owned(),
                target: target.to_owned(),
            });
    }
    let _ = stream
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await;
    let _ = stream.shutdown().await;
}

impl Drop for ConnectRecorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}
