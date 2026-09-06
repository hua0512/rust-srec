//! Bounded HTTP delivery with diagnostics that cannot retain endpoint credentials.

use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header::HeaderMap};
use tracing::debug;

use crate::{Error, Result};

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

pub(super) struct HttpDelivery {
    channel: &'static str,
    client: std::result::Result<Client, reqwest::Error>,
    timeout: Duration,
}

impl HttpDelivery {
    pub(super) fn new(channel: &'static str, timeout: Duration) -> Self {
        crate::utils::http_client::install_rustls_provider();
        Self {
            channel,
            client: Client::builder().build(),
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

    pub(super) fn request(&self, method: Method, url: &str) -> Result<RequestBuilder> {
        let client = self
            .client
            .as_ref()
            .map_err(|_| self.failure("HTTP client initialization failed"))?;
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

    use axum::{Router, body::Body, response::Response as ServerResponse};
    use tokio::net::TcpListener;

    use super::*;
    use crate::notification::channels::{
        DiscordChannel, DiscordConfig, GotifyChannel, GotifyConfig, NotificationChannel,
        WebhookChannel, WebhookConfig,
    };

    struct Server {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn server(router: Router) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/botPATH_SECRET/sendMessage?token=QUERY_SECRET",
            listener.local_addr().unwrap()
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
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
            let server = server(Router::new().fallback(move || async move {
                if !stall_body {
                    std::future::pending::<()>().await;
                }
                ServerResponse::builder()
                    .status(429)
                    .body(Body::from_stream(futures::stream::pending::<
                        std::result::Result<bytes::Bytes, std::io::Error>,
                    >()))
                    .unwrap()
            }))
            .await;
            let http = HttpDelivery::new("telegram", Duration::from_millis(100));
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                http.send(
                    http.request(Method::POST, &server.url)
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
                http.request(Method::POST, &later.url).unwrap(),
                RetryPolicy::None,
            )
            .await
            .unwrap();
        }
    }

    async fn server_ok() -> Server {
        server(Router::new().fallback(|| async { StatusCode::NO_CONTENT })).await
    }

    #[tokio::test]
    async fn request_errors_never_export_endpoint_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/botPATH_SECRET/sendMessage?token=QUERY_SECRET",
            listener.local_addr().unwrap()
        );
        drop(listener);
        let http = HttpDelivery::new("telegram", Duration::from_secs(2));
        for url in [endpoint.as_str(), "http://[PATH_SECRET/?token=QUERY_SECRET"] {
            let error = http
                .send(
                    http.request(Method::POST, url).unwrap(),
                    RetryPolicy::Telegram,
                )
                .await
                .unwrap_err();
            assert!(!format!("{error:?} {error}").contains("SECRET"));
        }
    }

    #[tokio::test]
    async fn notification_service_delivers_later_channel_after_timeout() {
        use crate::notification::channels::ChannelConfig;
        use crate::notification::events::NotificationEvent;
        use crate::notification::service::{NotificationService, NotificationServiceConfig};

        let delivered = Arc::new(AtomicUsize::new(0));
        let delivered_handler = delivered.clone();
        let server = server(
            Router::new()
                .route(
                    "/later",
                    axum::routing::post(move || {
                        let delivered = delivered_handler.clone();
                        async move {
                            delivered.fetch_add(1, Ordering::SeqCst);
                            StatusCode::NO_CONTENT
                        }
                    }),
                )
                .fallback(|| async { std::future::pending::<StatusCode>().await }),
        )
        .await;
        let mut later_url = reqwest::Url::parse(&server.url).unwrap();
        later_url.set_path("/later");
        later_url.set_query(None);
        let service = NotificationService::with_config(NotificationServiceConfig {
            max_retries: 1,
            ..Default::default()
        });
        for (id, url) in [
            ("stalled", server.url.clone()),
            ("later", later_url.to_string()),
        ] {
            service.add_channel(ChannelConfig::Webhook(WebhookConfig {
                id: Some(id.to_owned()),
                enabled: true,
                url,
                timeout_secs: 1,
                ..Default::default()
            }));
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            service.notify(NotificationEvent::SystemStartup {
                version: "test".to_owned(),
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(delivered.load(Ordering::SeqCst), 1);
        assert_eq!(service.stats().pending_count, 0);
    }

    #[tokio::test]
    async fn channels_do_not_export_echoed_credentials_on_http_failure() {
        let server = server(Router::new().fallback(|| async {
            (
                StatusCode::BAD_REQUEST,
                "https://host/PATH_SECRET?token=QUERY_SECRET Authorization: HEADER_SECRET",
            )
        }))
        .await;
        let channels: Vec<Box<dyn NotificationChannel>> = vec![
            Box::new(DiscordChannel::new(DiscordConfig {
                enabled: true,
                webhook_url: server.url.clone(),
                ..Default::default()
            })),
            Box::new(GotifyChannel::new(GotifyConfig {
                enabled: true,
                server_url: server.url.clone(),
                app_token: "QUERY_SECRET".into(),
                ..Default::default()
            })),
            Box::new(WebhookChannel::new(WebhookConfig {
                enabled: true,
                url: server.url.clone(),
                ..Default::default()
            })),
        ];
        for channel in channels {
            let error = channel.test().await.unwrap_err();
            assert!(error.to_string().contains("HTTP 400"));
            assert!(!format!("{error:?} {error}").contains("SECRET"));
        }
        let http = HttpDelivery::new("telegram", DEFAULT_TIMEOUT);
        let error = http
            .send(
                http.request(Method::POST, &server.url).unwrap(),
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
                let server = server(Router::new().fallback(move || {
                    let calls = server_calls.clone();
                    async move {
                        let attempt = calls.fetch_add(1, Ordering::SeqCst);
                        let status = if succeeds && attempt == 2 { 204 } else { 429 };
                        ServerResponse::builder()
                            .status(status)
                            .header("retry-after", "0")
                            .body(Body::from(r#"{"parameters":{"retry_after":0}}"#))
                            .unwrap()
                    }
                }))
                .await;
                let http = HttpDelivery::new("test", Duration::from_secs(2));
                let result = http
                    .send(
                        http.request(Method::POST, &server.url)
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
            let server = server(Router::new().fallback(move || {
                let body = body.clone();
                async move { (StatusCode::TOO_MANY_REQUESTS, body) }
            }))
            .await;
            let http = HttpDelivery::new("telegram", DEFAULT_TIMEOUT);
            let response = http
                .request(Method::POST, &server.url)
                .unwrap()
                .send()
                .await
                .unwrap();
            assert_eq!(telegram_retry_after(response).await.unwrap(), expected);
        }
    }
}
