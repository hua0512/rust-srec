use super::*;

#[tokio::test]
async fn web_push_debug_redacts_private_keys_cached_tokens_and_subscriptions() {
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
