use std::sync::Arc;
use std::time::Duration;

use crate::api::auth_service::{AuthConfig, AuthService};
use crate::api::auth_store::{
    SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
};
use crate::api::bootstrap::ApiServerHost;
use crate::api::jwt::JwtService;
use crate::api::rate_limit::LoginRateLimiter;
use crate::api::server::ApiServerConfig;
use crate::services::{ServiceContainer, ServiceContainerConfig};

/// The served API keys failed logins by the connection's peer address.
/// The router must be served with its connect info for `ClientIp` to see
/// that address; without it the per-address budget silently never
/// applies and one client can spread Argon2 work over any number of
/// usernames. An address budget of one makes the second failure, under
/// another username, the throttled one. The end-to-end suite cannot
/// reach this: the default budget is 100 failures, each a slow Argon2
/// verification.
#[tokio::test]
async fn the_served_api_budgets_failed_logins_per_peer_address() {
    let directory = tempfile::tempdir().unwrap();
    let (container, host, pool) = served_test_container(directory.path(), false).await;
    let auth = AuthService::new(
        Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone())),
        Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone())),
        Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool)),
        Arc::new(
            JwtService::new(
                "client-ip-test-secret-with-enough-entropy",
                "rust-srec",
                "rust-srec-api",
                None,
            )
            .unwrap(),
        ),
        AuthConfig::default(),
    )
    .with_login_rate_limiter(LoginRateLimiter::new(5, 1, Duration::from_secs(60), 8));
    let address = host
        .bind_and_spawn_api_server_for_tests(
            crate::backend::build_api_runtime(
                &container,
                crate::backend::build_config_backup(&container),
            )
            .unwrap(),
            Some(Arc::new(auth)),
            "test bridge",
            "test server",
        )
        .await
        .unwrap();

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let mut statuses = Vec::new();
    for username in ["alice", "bob"] {
        let response = client
            .post(format!("http://{address}/api/auth/login"))
            .json(&serde_json::json!({ "username": username, "password": "wrong-password-1" }))
            .send()
            .await
            .unwrap();
        statuses.push(response.status());
    }
    assert_eq!(
        statuses,
        [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ]
    );
    container.cancellation_token.cancel();
}

/// The desktop shell builds its container with local desktop mode on.
/// The served login must then answer the seeded `admin` / `admin123!`
/// exactly like a wrong password, which only holds if the container
/// hands the mode to the `AuthService` it builds.
#[tokio::test]
async fn local_desktop_mode_reaches_the_served_login() {
    let directory = tempfile::tempdir().unwrap();
    let (container, host, _pool) = served_test_container(directory.path(), true).await;
    let address = host
        .start_api_server_bound_with_jwt_secret(
            crate::backend::build_api_runtime(
                &container,
                crate::backend::build_config_backup(&container),
            )
            .unwrap(),
            "local-desktop-mode-test-secret-with-entropy".to_owned(),
        )
        .await
        .unwrap();

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let mut answers = Vec::new();
    for password in ["admin123!", "wrong-password-1"] {
        let response = client
            .post(format!("http://{address}/api/auth/login"))
            .json(&serde_json::json!({ "username": "admin", "password": password }))
            .send()
            .await
            .unwrap();
        answers.push((response.status(), response.text().await.unwrap()));
    }
    assert_eq!(answers[0].0, reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(answers[0], answers[1]);
    container.cancellation_token.cancel();
}

/// A container and API host on a fresh in-memory database, ready to start
/// the API on an ephemeral loopback port.
async fn served_test_container(
    log_dir: &std::path::Path,
    local_desktop_mode: bool,
) -> (ServiceContainer, ApiServerHost, sqlx::SqlitePool) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::with_full_config(
        pool.clone(),
        pool.clone(),
        ServiceContainerConfig {
            cache_ttl: Duration::from_secs(60),
            event_capacity: 8,
            download_config: crate::downloader::DownloadManagerConfig::default(),
            pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
        },
    )
    .await
    .unwrap();
    let host = ApiServerHost::new(
        ApiServerConfig {
            bind_address: "127.0.0.1".to_owned(),
            port: 0,
            enable_cors: false,
            body_limit: 1024 * 1024,
        },
        local_desktop_mode,
    )
    .unwrap();
    let (logging, _layer) = crate::logging::LoggingConfig::for_route_tests(log_dir.to_path_buf());
    assert!(container.logging_config.set(Arc::new(logging)).is_ok());
    (container, host, pool)
}
