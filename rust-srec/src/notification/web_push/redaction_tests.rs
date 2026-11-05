use super::*;

#[tokio::test]
async fn web_push_debug_redacts_private_keys_cached_tokens_and_subscriptions() {
    crate::utils::http_client::install_rustls_provider();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let private_key = [37; 32];
    let config = WebPushConfig {
        vapid_public_key_b64: "public-key-diagnostic".to_string(),
        vapid_private_key_raw: private_key,
        vapid_subject: "mailto:private-contact@example.com".to_string(),
    };
    let config_debug = format!("{config:?}");
    assert!(!config_debug.contains(&format!("{private_key:?}")));
    assert!(!config_debug.contains("private-contact"));
    assert!(config_debug.contains("public-key-diagnostic"));
    let service = WebPushService::new(pool.clone(), pool.clone(), config).unwrap();
    let jwt = CachedVapidJwt {
        jwt: "cached-jwt-credential".to_string(),
        exp_unix: 1234,
    };
    assert!(!format!("{jwt:?}").contains("cached-jwt-credential"));
    service
        .vapid_jwt_cache
        .insert("private-jwt-audience".to_string(), jwt);
    let mut cache = service.subscription_cache.write().await;
    *cache = Some((
        Instant::now(),
        vec![WebPushSubscriptionDbModel {
            id: "private-subscription-id".to_string(),
            user_id: "private-user-id".to_string(),
            endpoint: "https://push.example/endpoint-credential".to_string(),
            p256dh: "private-subscription-key".to_string(),
            auth: "subscription-auth-credential".to_string(),
            min_priority: 5,
            created_at: 0,
            updated_at: 0,
            next_attempt_at: None,
            last_429_at: None,
        }],
    ));
    // Debug must not inspect or wait for the locked cache.
    let jwt_guard = service
        .vapid_jwt_cache
        .get_mut("private-jwt-audience")
        .unwrap();
    for debug in [format!("{service:?}"), format!("{service:#?}")] {
        for secret in [
            "private-contact",
            "cached-jwt-credential",
            "private-jwt-audience",
            "private-subscription-id",
            "private-user-id",
            "endpoint-credential",
            "private-subscription-key",
            "subscription-auth-credential",
        ] {
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
        assert!(!debug.contains(&format!("{private_key:?}")));
        assert!(debug.contains("public-key-diagnostic"));
        assert!(debug.contains("vapid_jwt_cache"));
    }
    drop(jwt_guard);
    drop(cache);
    pool.close().await;
}

#[derive(Clone, Default)]
struct CapturedLog(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A failed delivery logs the push service origin, never the subscription
/// endpoint path, which identifies the subscriber.
#[tokio::test]
async fn failed_delivery_logs_only_the_push_service_origin() {
    crate::utils::http_client::install_rustls_provider();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let private = [37u8; 32];
    let public = SigningKey::from_bytes((&private).into())
        .unwrap()
        .verifying_key()
        .to_sec1_point(false);
    let mut service = WebPushService::new(
        pool.clone(),
        pool.clone(),
        WebPushConfig {
            vapid_public_key_b64: encode_b64url(public.as_bytes()),
            vapid_private_key_raw: private,
            vapid_subject: "mailto:test@example.com".to_string(),
        },
    )
    .unwrap();
    service.client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    *service.subscription_cache.write().await = Some((
        Instant::now(),
        vec![WebPushSubscriptionDbModel {
            id: "subscription".to_string(),
            user_id: "user".to_string(),
            endpoint: format!("{origin}/endpoint-credential"),
            p256dh: encode_b64url(public.as_bytes()),
            auth: encode_b64url(&[19; AUTH_SECRET_LEN]),
            min_priority: 0,
            created_at: 0,
            updated_at: 0,
            next_attempt_at: None,
            last_429_at: None,
        }],
    ));

    let captured = CapturedLog::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let event = NotificationEvent::SystemStartup {
        version: "test".into(),
        timestamp: Utc::now(),
    };
    tokio::time::timeout(Duration::from_secs(10), service.send_event(&event, None))
        .await
        .expect("delivery to a closed port must finish");

    let log = String::from_utf8_lossy(&captured.0.lock()).into_owned();
    assert!(log.contains("Web push delivery failed"), "{log}");
    assert!(log.contains(&origin), "{log}");
    assert!(!log.contains("endpoint-credential"), "{log}");
    pool.close().await;
}
