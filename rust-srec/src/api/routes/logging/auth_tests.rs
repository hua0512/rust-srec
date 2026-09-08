use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
use crate::api::auth_request::tests::{fixture, open_socket, wait_for_socket_close};
use crate::database::{
    models::ApiKeyAccessLevel,
    repositories::{SqlxConfigRepository, SqlxStreamerRepository},
};

fn route_state(
    fixture: &crate::api::auth_request::tests::AuthFixture,
    logging_config: crate::logging::LoggingConfig,
) -> LoggingRouteState {
    LoggingRouteState {
        auth_service: Some(fixture.service.clone()),
        config_service: Arc::new(crate::config::ConfigService::new(
            Arc::new(SqlxConfigRepository::new(
                fixture.pool.clone(),
                fixture.pool.clone(),
            )),
            Arc::new(SqlxStreamerRepository::new(
                fixture.pool.clone(),
                fixture.pool.clone(),
            )),
        )),
        logging_config: Arc::new(logging_config),
        logging_download_tokens: Arc::new(dashmap::DashMap::new()),
    }
}

#[tokio::test]
async fn logging_routes_require_full_keys_and_archive_route_rechecks_issuer() {
    let fixture = fixture().await;
    let (_, read) = fixture
        .service
        .create_api_key(
            &fixture.user_id,
            "logging-read",
            ApiKeyAccessLevel::ReadOnly,
            None,
        )
        .await
        .unwrap();
    let (key, full) = fixture
        .service
        .create_api_key(
            &fixture.user_id,
            "logging-full",
            ApiKeyAccessLevel::Full,
            None,
        )
        .await
        .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let (config, _layer) = crate::logging::LoggingConfig::for_route_tests(dir.path().to_owned());
    let state = route_state(&fixture, config);
    let tokens = state.logging_download_tokens.clone();
    let app = Router::new()
        .route("/", get(get_logging_config).put(update_logging_config))
        .route("/archive-token", get(get_archive_token))
        .with_state(state);
    for (method, path) in [("GET", "/"), ("PUT", "/"), ("GET", "/archive-token")] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("Authorization", format!("Bearer {read}"))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"filter":"debug"}"#))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/")
                .header("Authorization", format!("Bearer {full}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"filter":"["}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        update.status(),
        StatusCode::BAD_REQUEST,
        "full key reaches filter validation"
    );
    let issued = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/archive-token")
                .header("Authorization", format!("Bearer {full}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(issued.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = issued["token"].as_str().unwrap();
    let grant = tokens.get(token).unwrap();
    assert_eq!(
        grant.principal.as_ref().unwrap().api_key_id.as_deref(),
        Some(key.id.as_str())
    );
    drop(grant);
    let archives = Router::new()
        .route("/archive", get(download_logs_archive))
        .with_state(ArchiveRouteState {
            log_dir: dir.path().to_owned(),
            tokens: tokens.clone(),
            auth_service: Some(fixture.service.clone()),
            archives: Arc::new(LogArchiveService::new()),
        });
    fixture
        .service
        .revoke_api_key(&fixture.user_id, &key.id)
        .await
        .unwrap();
    assert_eq!(
        archives
            .oneshot(
                Request::builder()
                    .uri(format!("/archive?token={token}"))
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(!tokens.contains_key(token));
    fixture.pool.close().await;
}

#[tokio::test]
async fn logging_socket_rejects_read_keys_and_disconnects_downgraded_keys() {
    let fixture = fixture().await;
    let (_, read) = fixture
        .service
        .create_api_key(
            &fixture.user_id,
            "log-ws-read",
            ApiKeyAccessLevel::ReadOnly,
            None,
        )
        .await
        .unwrap();
    let (key, full) = fixture
        .service
        .create_api_key(
            &fixture.user_id,
            "log-ws-full",
            ApiKeyAccessLevel::Full,
            None,
        )
        .await
        .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let (config, _layer) = crate::logging::LoggingConfig::for_route_tests(dir.path().to_owned());
    let state = route_state(&fixture, config);
    let app = Router::new()
        .route("/stream", get(logging_stream_ws))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    open_socket(address, &format!("/stream?token={read}"), None, 403).await;
    open_socket(
        address,
        &format!("/stream?token={full}"),
        Some("Bearer invalid"),
        401,
    )
    .await;
    let socket = open_socket(address, &format!("/stream?token={full}"), None, 101).await;
    sqlx::query("UPDATE api_keys SET access_level = ? WHERE id = ?")
        .bind(ApiKeyAccessLevel::ReadOnly.as_str())
        .bind(&key.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    wait_for_socket_close(socket).await;
    drop(server);
    fixture.pool.close().await;
}

#[tokio::test]
async fn archive_grant_is_consumed_and_rejected_after_issuing_session_logout() {
    let fixture = fixture().await;
    let dir = tempfile::TempDir::new().unwrap();
    let (config, _layer) = crate::logging::LoggingConfig::for_route_tests(dir.path().to_owned());
    let state = route_state(&fixture, config);
    let tokens = state.logging_download_tokens.clone();
    let app = Router::new()
        .route("/archive-token", get(get_archive_token))
        .with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/archive-token")
                .header("Authorization", format!("Bearer {}", fixture.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = body["token"].as_str().unwrap();
    assert!(
        tokens
            .get(token)
            .unwrap()
            .principal
            .as_ref()
            .unwrap()
            .claims
            .sid
            .is_some()
    );
    let archives = Router::new()
        .route("/archive", get(download_logs_archive))
        .with_state(ArchiveRouteState {
            log_dir: dir.path().to_owned(),
            tokens: tokens.clone(),
            auth_service: Some(fixture.service.clone()),
            archives: Arc::new(LogArchiveService::new()),
        });
    fixture
        .service
        .logout(&fixture.refresh_token)
        .await
        .unwrap();
    for _ in 0..2 {
        let response = archives
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/archive?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(!tokens.contains_key(token));
    fixture.pool.close().await;
}
