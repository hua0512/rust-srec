//! Health endpoints: caller rules and the JSON their consumers (the frontend,
//! Docker healthchecks, the Watchtower pre-update hook) read.

use reqwest::header::AUTHORIZATION;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{Response, TestApp, WAIT, eventually};

const STATUSES: [&str; 4] = ["healthy", "degraded", "unhealthy", "unknown"];

fn is_optional(value: &Value, check: fn(&Value) -> bool) -> bool {
    value.is_null() || check(value)
}

/// Wait until the background refresh has published a populated snapshot.
async fn populated_health(app: &TestApp, token: &str) -> Response {
    eventually(WAIT, "a populated health snapshot", || async {
        let response = app.get("/api/health", token).await;
        let populated = response.status == StatusCode::OK
            && response.body["status"] != "unknown"
            && response.body["components"]
                .as_array()
                .is_some_and(|components| !components.is_empty());
        populated.then_some(response)
    })
    .await
}

/// `/ready` answers from the same snapshot as `/api/health`.
async fn assert_ready_matches(app: &TestApp, token: &str, overall: &Value) {
    let ready = app.get("/api/health/ready", token).await;
    if overall == "healthy" || overall == "degraded" {
        assert_eq!(
            (ready.status, ready.text.as_str()),
            (StatusCode::OK, "ready")
        );
    } else {
        assert_eq!(
            (ready.status, ready.text.as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, "not ready"),
            "overall status {overall}"
        );
    }
}

#[tokio::test]
async fn health_requires_a_credential_and_reports_every_core_component() {
    let app = TestApp::start().await;

    for path in ["/api/health", "/api/health/ready"] {
        let response = app.send(Method::GET, path, None, None).await;
        assert_eq!(
            (response.status, response.error_code()),
            (StatusCode::UNAUTHORIZED, Some("UNAUTHORIZED")),
            "{path}"
        );
    }

    let token = app.admin_token().await;
    let health = populated_health(&app, &token).await;
    let body = &health.body;
    assert!(
        STATUSES.contains(&body["status"].as_str().unwrap_or_default()),
        "{}",
        health.text
    );
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["uptime_secs"].is_u64());
    assert!(body["cpu_usage"].is_number());
    assert!(body["memory_usage"].is_number());
    let components = body["components"].as_array().expect("components");
    for component in components {
        assert!(component["name"].is_string(), "{component}");
        assert!(
            STATUSES.contains(&component["status"].as_str().unwrap_or_default()),
            "{component}"
        );
        assert!(
            is_optional(&component["message"], Value::is_string),
            "{component}"
        );
        assert!(
            is_optional(&component["last_check"], Value::is_string),
            "{component}"
        );
        assert!(
            is_optional(&component["check_duration_ms"], Value::is_number),
            "{component}"
        );
        assert!(
            is_optional(&component["disk"], Value::is_object),
            "{component}"
        );
    }
    for name in [
        "database",
        "download_manager",
        "pipeline_manager",
        "scheduler",
    ] {
        let component = components
            .iter()
            .find(|component| component["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {}", health.text));
        assert_eq!(component["status"], "healthy", "{component}");
    }
    assert_ready_matches(&app, &token, &body["status"]).await;

    // Read-only keys may monitor health; a revoked one may not.
    let key = app.create_api_key(&token, "monitor", "read_only").await;
    let read_key = key.raw;
    let health = app
        .get("/api/health", &read_key)
        .await
        .expect_status(StatusCode::OK);
    assert_ready_matches(&app, &read_key, &health.body["status"]).await;
    app.delete(&format!("/api/auth/api-keys/{}", key.id), &token)
        .await
        .expect_status(StatusCode::OK);
    for path in ["/api/health", "/api/health/ready"] {
        assert_eq!(
            app.get(path, &read_key).await.status,
            StatusCode::UNAUTHORIZED,
            "{path} with a revoked key"
        );
    }

    app.shutdown().await;
}

#[tokio::test]
async fn liveness_and_idle_are_public_and_idle_reports_processing_jobs() {
    let app = TestApp::start().await;

    let get_public = |path: &'static str, authorization: Option<&'static str>| {
        let app = &app;
        async move {
            let mut request = app.client().get(app.url(path));
            if let Some(value) = authorization {
                request = request.header(AUTHORIZATION, value);
            }
            TestApp::read(request.send().await.expect("send the request")).await
        }
    };

    // Docker healthchecks call this without, or with a stale, credential.
    for authorization in [None, Some("Bearer not-a-token"), Some("Basic x")] {
        let live = get_public("/api/health/live", authorization)
            .await
            .expect_status(StatusCode::OK);
        let body = live.body.as_object().expect("liveness body");
        assert_eq!(body.len(), 2, "{}", live.text);
        assert_eq!(body["status"], "alive");
        assert!(body["uptime_secs"].is_u64());

        let idle = get_public("/api/health/idle", authorization)
            .await
            .expect_status(StatusCode::OK);
        assert_eq!(
            idle.body,
            json!({
                "idle": true,
                "active_recordings": 0,
                "pending_recordings": 0,
                "processing_jobs": 0,
            })
        );
    }

    app.run_sql(
        "INSERT INTO job(id, job_type, status, config, state, created_at, updated_at) \
         VALUES ('e2e-busy', 'remux', 'PROCESSING', '{}', '{}', 0, 0)",
    )
    .await;
    let busy = get_public("/api/health/idle", None)
        .await
        .expect_status(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        busy.body,
        json!({
            "idle": false,
            "active_recordings": 0,
            "pending_recordings": 0,
            "processing_jobs": 1,
        })
    );

    app.run_sql("UPDATE job SET status = 'COMPLETED' WHERE id = 'e2e-busy'")
        .await;
    let idle = get_public("/api/health/idle", None)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(idle.body["idle"], true);
    assert_eq!(idle.body["processing_jobs"], 0);

    app.shutdown().await;
}
