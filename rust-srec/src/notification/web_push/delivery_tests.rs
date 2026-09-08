use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::post;
use tokio_util::task::AbortOnDropHandle;

use crate::database::models::UserDbModel;
use crate::database::repositories::{SqlxUserRepository, UserRepository};

use super::*;

struct Fixture {
    service: WebPushService,
    endpoint: String,
    status: Arc<AtomicU16>,
    hits: Arc<AtomicUsize>,
    metrics: Arc<crate::metrics::MetricsCollector>,
    _server: AbortOnDropHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        crate::utils::http_client::install_rustls_provider();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let mut user = UserDbModel::new("push-test", "unused", Vec::new());
        user.id = "push-test".into();
        SqlxUserRepository::new(pool.clone(), pool.clone())
            .create(&user)
            .await
            .unwrap();
        let status = Arc::new(AtomicU16::new(201));
        let hits = Arc::new(AtomicUsize::new(0));
        let (response_status, response_hits) = (status.clone(), hits.clone());
        let application = Router::new().route(
            "/push",
            post(move || {
                let (status, hits) = (response_status.clone(), response_hits.clone());
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap(),
                        [("retry-after", "60")],
                        "fixture",
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/push", listener.local_addr().unwrap());
        let server = AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, application).await.unwrap();
        }));
        let private = [37u8; 32];
        let signing = SigningKey::from_bytes((&private).into()).unwrap();
        let public = signing.verifying_key().to_sec1_point(false);
        let mut service = WebPushService::new(
            pool.clone(),
            pool,
            WebPushConfig {
                vapid_public_key_b64: encode_b64url(public.as_bytes()),
                vapid_private_key_raw: private,
                vapid_subject: "mailto:test@example.com".into(),
            },
        )
        .unwrap();
        service.client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
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
        let metrics = Arc::new(crate::metrics::MetricsCollector::new());
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
        timestamp: Utc::now(),
    }
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
            assert_eq!(fixture.metrics.snapshot().web_push_stale_deleted_total, 1);
        }
    })
    .await
    .expect("local stale-subscription scenario must finish");
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
        assert_eq!(fixture.metrics.snapshot().web_push_skipped_backoff_total, 1);

        sqlx::query("UPDATE web_push_subscription SET next_attempt_at = ? WHERE endpoint = ?")
            .bind(Utc::now().timestamp_millis() - 1)
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
        assert_eq!(fixture.metrics.snapshot().web_push_sent_total, 1);
        assert_eq!(fixture.metrics.snapshot().web_push_throttled_total, 1);
    })
    .await
    .expect("local throttling scenario must finish");
}

#[tokio::test]
async fn failed_stale_deletion_keeps_cache_and_does_not_report_deleted() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        fixture.service.list_all_subscriptions_cached().await.unwrap();
        sqlx::query("CREATE TRIGGER reject_push_delete BEFORE DELETE ON web_push_subscription BEGIN SELECT RAISE(ABORT, 'fixture deletion failure'); END")
            .execute(&fixture.service.write_pool).await.unwrap();
        fixture.status.store(410, Ordering::SeqCst);
        assert!(fixture.service.send_to_subscription(&fixture.row().await, &event(), None).await.is_err());
        assert_eq!(fixture.service.list_all_subscriptions().await.unwrap().len(), 1);
        assert!(fixture.service.subscription_cache.read().await.is_some());
        assert_eq!(fixture.metrics.snapshot().web_push_stale_deleted_total, 0);
    }).await.expect("local persistence failure scenario must finish");
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
