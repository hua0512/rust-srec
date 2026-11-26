//! Bounded HTTP delivery with diagnostics that cannot retain endpoint credentials.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response, StatusCode, header::HeaderMap};
use tracing::{debug, warn};

use srec_core::domain::proxy::{ProxyRoute, ProxyRouteResolver, ProxyTarget};
use srec_core::utils::http_client::service_client;

use crate::{NotifyError as Error, NotifyResult as Result};

pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_BODY_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryPolicy {
    None,
    Discord,
    Telegram,
}

/// How a channel finds the connection its deliveries take. Every HTTP
/// channel is built with one.
#[derive(Clone)]
pub enum DeliveryRoute {
    /// Always this connection, for channels configured outside the
    /// database.
    Fixed(ProxyTarget),
    /// A stored channel's own route, kept with the settings it was loaded
    /// with so a delivery never pairs one revision's destination with
    /// another's route. Inheriting and the saved proxy's address and login
    /// are resolved before each delivery, so edits to the global route or
    /// to a saved proxy apply to the next one.
    Stored {
        routes: Arc<dyn ProxyRouteResolver>,
        route: ProxyRoute,
    },
}

impl DeliveryRoute {
    /// The proxy the environment configures, if any.
    pub fn environment() -> Self {
        Self::Fixed(ProxyTarget::System)
    }
}

pub(super) struct HttpDelivery {
    channel: &'static str,
    route: DeliveryRoute,
    timeout: Duration,
}

impl HttpDelivery {
    pub(super) fn new(channel: &'static str, timeout: Duration, route: DeliveryRoute) -> Self {
        srec_core::utils::http_client::install_rustls_provider();
        Self {
            channel,
            route,
            timeout: if timeout.is_zero() {
                DEFAULT_TIMEOUT
            } else {
                timeout.min(MAX_TIMEOUT)
            },
        }
    }

    fn failure(&self, message: impl Into<String>) -> Error {
        Error::NotificationDelivery {
            channel: self.channel,
            message: message.into(),
        }
    }

    /// A request on the channel's current route. A route naming a saved
    /// proxy that no longer exists fails rather than connecting directly.
    pub(super) async fn request(&self, method: Method, url: &str) -> Result<RequestBuilder> {
        let client = match &self.route {
            DeliveryRoute::Fixed(target) => service_client(target)
                .map_err(|_| self.failure("HTTP client initialization failed"))?,
            DeliveryRoute::Stored { routes, route } => {
                let target = routes.target(route).await.map_err(|error| {
                    warn!(channel = self.channel, %error, "Notification proxy route unavailable");
                    self.failure(format!("proxy route unavailable: {error}"))
                })?;
                service_client(&target)
                    .map_err(|_| self.failure("HTTP client initialization failed"))?
            }
        };
        Ok(client.request(method, url).timeout(self.timeout))
    }

    pub(super) async fn send(&self, request: RequestBuilder, retry: RetryPolicy) -> Result<()> {
        for attempt in 1..=MAX_ATTEMPTS {
            let request = request
                .try_clone()
                .ok_or_else(|| self.failure("request cannot be retried"))?;
            // Include response-body parsing in the deadline, including slow 429 bodies.
            let delay = tokio::time::timeout(self.timeout, self.attempt(request, retry))
                .await
                .map_err(|_| self.failure("request timed out"))??;
            let Some(delay) = delay else {
                return Ok(());
            };
            if attempt == MAX_ATTEMPTS {
                return Err(self.failure("rate limit exceeded after 3 attempts"));
            }
            debug!(
                channel = self.channel,
                attempt,
                ?delay,
                "Notification rate limited"
            );
            tokio::time::sleep(delay).await;
        }
        Err(self.failure("rate limit exceeded"))
    }

    async fn attempt(
        &self,
        request: RequestBuilder,
        retry: RetryPolicy,
    ) -> Result<Option<Duration>> {
        let response = request
            .send()
            .await
            .map_err(|error| self.request_failure(error))?;
        let status = response.status();
        if status.is_success() {
            return Ok(None);
        }
        if status != StatusCode::TOO_MANY_REQUESTS || retry == RetryPolicy::None {
            // Servers may echo the request URL, headers, or payload in their error body.
            return Err(self.failure(format!("HTTP {}", status.as_u16())));
        }
        let delay = match retry {
            RetryPolicy::Discord => retry_headers(response.headers()),
            RetryPolicy::Telegram => telegram_retry_after(response)
                .await
                .map_err(|error| self.request_failure(error))?,
            RetryPolicy::None => None,
        };
        Ok(Some(delay.unwrap_or(Duration::from_secs(1))))
    }

    fn request_failure(&self, error: reqwest::Error) -> Error {
        // URLs, nested errors and echoed bodies can all contain credentials.
        // Export only categories; callers persist and log these messages.
        self.failure(if error.is_timeout() {
            "request timed out"
        } else if error.is_connect() {
            "connection failed"
        } else if error.is_builder() {
            "invalid request configuration"
        } else {
            "request failed"
        })
    }
}

fn retry_seconds(seconds: f64) -> Option<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(seconds.min(MAX_RETRY_DELAY.as_secs_f64())).ok()
}

fn retry_headers(headers: &HeaderMap) -> Option<Duration> {
    ["retry-after", "x-ratelimit-reset-after"]
        .into_iter()
        .find_map(|name| retry_seconds(headers.get(name)?.to_str().ok()?.parse().ok()?))
}

async fn telegram_retry_after(
    mut response: Response,
) -> std::result::Result<Option<Duration>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_RETRY_BODY_BYTES {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    let delay = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|body| retry_seconds(body.get("parameters")?.get("retry_after")?.as_f64()?));
    Ok(delay)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::channels::{
        DiscordChannel, DiscordConfig, GotifyChannel, GotifyConfig, NotificationChannel,
        WebhookChannel, WebhookConfig,
    };

    enum TestResponse {
        StallHeaders,
        Stall429Body,
        Reply {
            status: u16,
            headers: Vec<(&'static str, String)>,
            body: String,
        },
    }

    struct Server {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> bool {
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        let header_end = loop {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                    if buf.len() > 64 * 1024 {
                        return false;
                    }
                }
            }
        };
        let headers_str = String::from_utf8_lossy(&buf[..header_end]);
        let content_length = headers_str
            .lines()
            .skip(1)
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    }

    async fn server<F>(handler: F) -> Server
    where
        F: Fn() -> TestResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/botPATH_SECRET/sendMessage?token=QUERY_SECRET",
            listener.local_addr().unwrap()
        );
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    if !read_http_request(&mut stream).await {
                        return;
                    }
                    match handler() {
                        TestResponse::StallHeaders => {
                            std::future::pending::<()>().await;
                        }
                        TestResponse::Stall429Body => {
                            let _ = stream
                                .write_all(
                                    b"HTTP/1.1 429 Too Many Requests\r\nTransfer-Encoding: chunked\r\n\r\n",
                                )
                                .await;
                            let _ = stream.flush().await;
                            std::future::pending::<()>().await;
                        }
                        TestResponse::Reply {
                            status,
                            headers,
                            body,
                        } => {
                            let reason = match status {
                                204 => "No Content",
                                400 => "Bad Request",
                                429 => "Too Many Requests",
                                _ => "OK",
                            };
                            let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
                            for (name, value) in headers {
                                response.push_str(&format!("{name}: {value}\r\n"));
                            }
                            response.push_str(&format!(
                                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            ));
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.write_all(body.as_bytes()).await;
                            let _ = stream.shutdown().await;
                        }
                    }
                });
            }
        });
        Server { url, task }
    }

    #[test]
    fn retry_delays_reject_invalid_values_and_cap_large_values() {
        for value in ["-1", "NaN", "inf", "-inf", "1e999", "invalid", ""] {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", value.parse().unwrap());
            assert_eq!(retry_headers(&headers), None, "{value}");
            headers.insert("x-ratelimit-reset-after", "0.25".parse().unwrap());
            assert_eq!(retry_headers(&headers), Some(Duration::from_millis(250)));
        }
        for value in ["18446744073709551615", "1e308", "30", "31"] {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", value.parse().unwrap());
            assert_eq!(retry_headers(&headers), Some(MAX_RETRY_DELAY));
        }
    }

    #[tokio::test]
    async fn stalled_headers_and_retry_body_time_out_then_later_delivery_succeeds() {
        for stall_body in [false, true] {
            let server = server(move || {
                if !stall_body {
                    TestResponse::StallHeaders
                } else {
                    TestResponse::Stall429Body
                }
            })
            .await;
            let http = HttpDelivery::new(
                "telegram",
                Duration::from_millis(100),
                DeliveryRoute::environment(),
            );
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                http.send(
                    http.request(Method::POST, &server.url)
                        .await
                        .unwrap()
                        .json(&serde_json::json!({})),
                    RetryPolicy::Telegram,
                ),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains("timed out"));
            assert!(!format!("{error:?}").contains("SECRET"));

            let later = server_ok().await;
            http.send(
                http.request(Method::POST, &later.url).await.unwrap(),
                RetryPolicy::None,
            )
            .await
            .unwrap();
        }
    }

    async fn server_ok() -> Server {
        server(|| TestResponse::Reply {
            status: 204,
            headers: Vec::new(),
            body: String::new(),
        })
        .await
    }

    #[tokio::test]
    async fn request_errors_never_export_endpoint_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/botPATH_SECRET/sendMessage?token=QUERY_SECRET",
            listener.local_addr().unwrap()
        );
        drop(listener);
        let http = HttpDelivery::new(
            "telegram",
            Duration::from_secs(2),
            DeliveryRoute::environment(),
        );
        for url in [endpoint.as_str(), "http://[PATH_SECRET/?token=QUERY_SECRET"] {
            let error = http
                .send(
                    http.request(Method::POST, url).await.unwrap(),
                    RetryPolicy::Telegram,
                )
                .await
                .unwrap_err();
            assert!(!format!("{error:?} {error}").contains("SECRET"));
        }
    }

    #[tokio::test]
    async fn channels_do_not_export_echoed_credentials_on_http_failure() {
        let server = server(|| TestResponse::Reply {
            status: 400,
            headers: Vec::new(),
            body: "https://host/PATH_SECRET?token=QUERY_SECRET Authorization: HEADER_SECRET"
                .to_owned(),
        })
        .await;
        let channels: Vec<Box<dyn NotificationChannel>> = vec![
            Box::new(DiscordChannel::new(
                DiscordConfig {
                    enabled: true,
                    webhook_url: server.url.clone(),
                    ..Default::default()
                },
                DeliveryRoute::environment(),
            )),
            Box::new(GotifyChannel::new(
                GotifyConfig {
                    enabled: true,
                    server_url: server.url.clone(),
                    app_token: "QUERY_SECRET".into(),
                    ..Default::default()
                },
                DeliveryRoute::environment(),
            )),
            Box::new(WebhookChannel::new(
                WebhookConfig {
                    enabled: true,
                    url: server.url.clone(),
                    ..Default::default()
                },
                DeliveryRoute::environment(),
            )),
        ];
        for channel in channels {
            let error = channel.test().await.unwrap_err();
            assert!(error.to_string().contains("HTTP 400"));
            assert!(!format!("{error:?} {error}").contains("SECRET"));
        }
        let http = HttpDelivery::new("telegram", DEFAULT_TIMEOUT, DeliveryRoute::environment());
        let error = http
            .send(
                http.request(Method::POST, &server.url).await.unwrap(),
                RetryPolicy::Telegram,
            )
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SECRET"));
    }

    #[tokio::test]
    async fn rate_limited_requests_retry_and_stop_at_attempt_limit() {
        for policy in [RetryPolicy::Discord, RetryPolicy::Telegram] {
            for succeeds in [true, false] {
                let calls = Arc::new(AtomicUsize::new(0));
                let server_calls = calls.clone();
                let server = server(move || {
                    let attempt = server_calls.fetch_add(1, Ordering::SeqCst);
                    let status = if succeeds && attempt == 2 { 204 } else { 429 };
                    TestResponse::Reply {
                        status,
                        headers: vec![("retry-after", "0".to_owned())],
                        body: if status == 204 {
                            String::new()
                        } else {
                            r#"{"parameters":{"retry_after":0}}"#.to_owned()
                        },
                    }
                })
                .await;
                let http =
                    HttpDelivery::new("test", Duration::from_secs(2), DeliveryRoute::environment());
                let result = http
                    .send(
                        http.request(Method::POST, &server.url)
                            .await
                            .unwrap()
                            .json(&serde_json::json!({"text":"hello"})),
                        policy,
                    )
                    .await;
                assert_eq!(result.is_ok(), succeeds);
                assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS);
            }
        }
    }

    #[tokio::test]
    async fn telegram_retry_body_is_bounded_and_validated() {
        for (body, expected) in [
            (r#"{"parameters":{"retry_after":-1}}"#.to_owned(), None),
            (r#"{"parameters":{"retry_after":"NaN"}}"#.to_owned(), None),
            (
                r#"{"parameters":{"retry_after":18446744073709551615}}"#.to_owned(),
                Some(MAX_RETRY_DELAY),
            ),
            (
                r#"{"parameters":{"retry_after":0.25}}"#.to_owned(),
                Some(Duration::from_millis(250)),
            ),
            ("x".repeat(MAX_RETRY_BODY_BYTES + 1), None),
            ("not json".to_owned(), None),
        ] {
            let server = server(move || TestResponse::Reply {
                status: 429,
                headers: Vec::new(),
                body: body.clone(),
            })
            .await;
            let http = HttpDelivery::new("telegram", DEFAULT_TIMEOUT, DeliveryRoute::environment());
            let response = http
                .request(Method::POST, &server.url)
                .await
                .unwrap()
                .send()
                .await
                .unwrap();
            assert_eq!(telegram_retry_after(response).await.unwrap(), expected);
        }
    }
}
