use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::task::AbortOnDropHandle;

use super::*;

#[derive(Default)]
struct TestWebPushMetrics {
    sent: AtomicU64,
    failed: AtomicU64,
    throttled: AtomicU64,
    stale_deleted: AtomicU64,
    skipped_backoff: AtomicU64,
}

impl WebPushMetrics for TestWebPushMetrics {
    fn record_web_push_sent(&self, _duration_ms: u64) {
        self.sent.fetch_add(1, Ordering::SeqCst);
    }

    fn record_web_push_failed(&self) {
        self.failed.fetch_add(1, Ordering::SeqCst);
    }

    fn record_web_push_throttled(&self) {
        self.throttled.fetch_add(1, Ordering::SeqCst);
    }

    fn record_web_push_stale_deleted(&self) {
        self.stale_deleted.fetch_add(1, Ordering::SeqCst);
    }

    fn record_web_push_skipped_backoff(&self) {
        self.skipped_backoff.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
struct CapturedLog(Arc<parking_lot::Mutex<Vec<u8>>>);

impl CapturedLog {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock()).into_owned()
    }
}

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

async fn read_http_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
                if buf.len() > 64 * 1024 {
                    return None;
                }
            }
        }
    };
    let headers_str = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let target = headers_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
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
    Some(target)
}

async fn serve_http<F>(path_suffix: &str, handler: F) -> (String, AbortOnDropHandle<()>)
where
    F: Fn(&str) -> (u16, Vec<(&'static str, String)>, &'static str) + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}{}", listener.local_addr().unwrap(), path_suffix);
    let handler = Arc::new(handler);
    let server = AbortOnDropHandle::new(tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let Some(target) = read_http_request_target(&mut stream).await else {
                    return;
                };
                let (status, headers, body) = handler(&target);
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    307 => "Temporary Redirect",
                    404 => "Not Found",
                    410 => "Gone",
                    429 => "Too Many Requests",
                    _ => "Error",
                };
                let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str(&format!(
                    "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ));
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    }));
    (endpoint, server)
}

struct Fixture {
    service: WebPushService,
    endpoint: String,
    status: Arc<AtomicU16>,
    hits: Arc<AtomicUsize>,
    metrics: Arc<TestWebPushMetrics>,
    _server: AbortOnDropHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        srec_core::utils::http_client::install_rustls_provider();
        let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        srec_sqlite::run_migrations(&pool).await.unwrap();
        let now_ms = jiff::Timestamp::now().as_millisecond();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, roles, is_active, must_change_password, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("push-test")
        .bind("push-test")
        .bind("unused")
        .bind("[]")
        .bind(true)
        .bind(false)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&pool)
        .await
        .unwrap();
        let status = Arc::new(AtomicU16::new(201));
        let hits = Arc::new(AtomicUsize::new(0));
        let (response_status, response_hits) = (status.clone(), hits.clone());
        let (endpoint, server) = serve_http("/push", move |target| {
            if target != "/push" {
                return (404, Vec::new(), "not found");
            }
            response_hits.fetch_add(1, Ordering::SeqCst);
            (
                response_status.load(Ordering::SeqCst),
                vec![("retry-after", "60".to_owned())],
                "fixture",
            )
        })
        .await;
        let private = [37u8; 32];
        let signing = SigningKey::from_bytes((&private).into()).unwrap();
        let public = signing.verifying_key().to_sec1_point(false);
        let service = WebPushService::new(
            pool.clone(),
            pool,
            WebPushConfig {
                vapid_public_key_b64: encode_b64url(public.as_bytes()),
                vapid_private_key_raw: private,
                vapid_subject: "mailto:test@example.com".into(),
            },
        )
        .unwrap();
        service
            .upsert_subscription(
                "push-test",
                &endpoint,
                &encode_b64url(public.as_bytes()),
                &encode_b64url(&[19; AUTH_SECRET_LEN]),
                NotificationPriority::Low,
            )
            .await
            .unwrap();
        let metrics = Arc::new(TestWebPushMetrics::default());
        service.set_metrics_collector(metrics.clone());
        Self {
            service,
            endpoint,
            status,
            hits,
            metrics,
            _server: server,
        }
    }

    async fn row(&self) -> WebPushSubscriptionDbModel {
        self.service
            .list_all_subscriptions()
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }
}

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "test".into(),
        timestamp: jiff::Timestamp::now(),
    }
}

/// A failed delivery logs the push service origin, never the subscription
/// endpoint path, which identifies the subscriber.
#[tokio::test]
async fn failed_delivery_logs_only_the_push_service_origin() {
    let Fixture {
        service,
        endpoint,
        _server,
        ..
    } = Fixture::new().await;
    // Stopping the server leaves the endpoint refusing connections.
    drop(_server);
    let path = Url::parse(&endpoint).unwrap().path().to_owned();
    let origin = endpoint.strip_suffix(&path).unwrap().to_owned();

    let captured = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    tokio::time::timeout(Duration::from_secs(10), service.send_event(&event(), None))
        .await
        .expect("delivery to a refused connection must finish");

    let log = captured.contents();
    assert!(log.contains("Web push delivery failed"), "{log}");
    assert!(log.contains(&origin), "{log}");
    assert!(!log.contains(&path), "{log}");
}

#[tokio::test]
async fn stale_http_responses_delete_rows_and_invalidate_warm_cache() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for status in [404, 410] {
            let fixture = Fixture::new().await;
            assert_eq!(
                fixture
                    .service
                    .list_all_subscriptions_cached()
                    .await
                    .unwrap()
                    .len(),
                1
            );
            fixture.status.store(status, Ordering::SeqCst);
            fixture.service.send_event(&event(), None).await;
            assert!(
                fixture
                    .service
                    .list_all_subscriptions()
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(fixture.service.subscription_cache.read().await.is_none());
            fixture.service.send_event(&event(), None).await;
            assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.metrics.stale_deleted.load(Ordering::SeqCst), 1);
        }
    })
    .await
    .expect("local stale-subscription scenario must finish");
}

/// When deleting a 404/410 subscription fails, the cached list and the
/// stale-deletion metric must not claim it was removed.
#[tokio::test]
async fn failed_stale_deletion_keeps_cache_and_does_not_report_deleted() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        fixture
            .service
            .list_all_subscriptions_cached()
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_push_delete BEFORE DELETE ON web_push_subscription \
             BEGIN SELECT RAISE(ABORT, 'fixture deletion failure'); END",
        )
        .execute(&fixture.service.write_pool)
        .await
        .unwrap();
        fixture.status.store(410, Ordering::SeqCst);
        assert!(
            fixture
                .service
                .send_to_subscription(
                    &fixture.service.client().await.unwrap(),
                    &fixture.row().await,
                    &event(),
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(
            fixture
                .service
                .list_all_subscriptions()
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(fixture.service.subscription_cache.read().await.is_some());
        assert_eq!(fixture.metrics.stale_deleted.load(Ordering::SeqCst), 0);
    })
    .await
    .expect("local persistence failure scenario must finish");
}

#[tokio::test]
async fn throttling_skips_http_and_first_success_clears_persisted_backoff() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        fixture.status.store(429, Ordering::SeqCst);
        fixture.service.send_event(&event(), None).await;
        let row = fixture.row().await;
        assert!(row.last_429_at.is_some());
        assert!(row.next_attempt_at.unwrap() >= row.last_429_at.unwrap() + 60_000);
        fixture.service.send_event(&event(), None).await;
        assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.skipped_backoff.load(Ordering::SeqCst), 1);

        sqlx::query("UPDATE web_push_subscription SET next_attempt_at = ? WHERE endpoint = ?")
            .bind(jiff::Timestamp::now().as_millisecond() - 1)
            .bind(&fixture.endpoint)
            .execute(&fixture.service.write_pool)
            .await
            .unwrap();
        *fixture.service.subscription_cache.write().await = None;
        fixture.status.store(201, Ordering::SeqCst);
        fixture.service.send_event(&event(), Some("event-id")).await;
        let row = fixture.row().await;
        assert!(row.next_attempt_at.is_none());
        assert!(row.last_429_at.is_none());
        assert!(fixture.service.subscription_cache.read().await.is_none());
        assert_eq!(fixture.hits.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.metrics.sent.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.throttled.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("local throttling scenario must finish");
}

#[test]
fn payload_caps_count_utf8_and_json_escaping_including_fallback_metadata() {
    for text in [
        "🎥中文".repeat(600),
        "\u{0001}\"\\".repeat(600),
        "\u{0001}".repeat(2000),
    ] {
        let mut payload = WebPushPayload::from_event(&event(), Some("event-id"));
        payload.title = text.clone();
        payload.body = text;
        let bytes = payload.into_bytes_capped(MAX_PAYLOAD_BYTES).unwrap();
        assert!(bytes.len() <= MAX_PAYLOAD_BYTES);
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded["event_log_id"], "event-id");
        assert!(decoded["title"].is_string());
    }
    let payload = WebPushPayload::from_event(&event(), Some(&"界".repeat(MAX_PAYLOAD_BYTES)));
    assert!(payload.into_bytes_capped(MAX_PAYLOAD_BYTES).is_err());
    assert!(
        WebPushPayload::from_event(&event(), None)
            .into_bytes_capped(1)
            .is_err()
    );
    let payload = WebPushPayload::from_event(&event(), Some("id"));
    let exact = serde_json::to_vec(&payload).unwrap().len();
    assert_eq!(payload.into_bytes_capped(exact).unwrap().len(), exact);
}

#[tokio::test]
async fn endpoint_validation_blocks_private_targets_and_delivery_refuses_redirects() {
    for allowed in [
        "http://localhost:8080/push",
        "http://127.0.0.1:8080/push",
        "http://[::1]:8080/push",
        "https://8.8.8.8/push",
    ] {
        validate_push_endpoint(allowed, false, false)
            .await
            .unwrap_or_else(|e| panic!("expected {allowed} to be allowed, got {e}"));
    }

    for rejected in [
        "https://169.254.169.254/latest/meta-data",
        "https://10.0.0.1/push",
        "https://172.16.0.1/push",
        "https://192.168.1.10/push",
        "https://sub.localhost/push",
        "https://user:pass@8.8.8.8/push",
        "http://8.8.8.8/push",
    ] {
        assert!(
            validate_push_endpoint(rejected, false, false)
                .await
                .is_err(),
            "expected {rejected} to be rejected"
        );
    }

    let ssrf_hits = Arc::new(AtomicUsize::new(0));
    let ssrf_hits_handler = ssrf_hits.clone();
    let (target_url, _target_server) = serve_http("/internal", move |target| {
        if target == "/internal" {
            ssrf_hits_handler.fetch_add(1, Ordering::SeqCst);
            (200, Vec::new(), "")
        } else {
            (404, Vec::new(), "")
        }
    })
    .await;

    let redirect_target = target_url.clone();
    let (redirect_endpoint, _redirect_server) = serve_http("/push", move |target| {
        if target == "/push" {
            (
                307,
                vec![("location", redirect_target.clone())],
                "redirecting",
            )
        } else {
            (404, Vec::new(), "")
        }
    })
    .await;

    let fixture = Fixture::new().await;
    fixture
        .service
        .unsubscribe("push-test", &fixture.endpoint)
        .await
        .unwrap();
    let private = [37u8; 32];
    let signing = SigningKey::from_bytes((&private).into()).unwrap();
    let public = signing.verifying_key().to_sec1_point(false);
    fixture
        .service
        .upsert_subscription(
            "push-test",
            &redirect_endpoint,
            &encode_b64url(public.as_bytes()),
            &encode_b64url(&[19; AUTH_SECRET_LEN]),
            NotificationPriority::Low,
        )
        .await
        .unwrap();

    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.service.send_event(&event(), None),
    )
    .await
    .expect("redirect refusal must finish");

    assert_eq!(ssrf_hits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.failed.load(Ordering::SeqCst), 1);
}
