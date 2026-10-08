use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use axum::{Extension, Router};
use serde_json::{Value, json};
use tower::ServiceExt;
use utoipa::OpenApi;

use crate::api::jwt::Claims;
use crate::api::openapi::ApiDoc;
use crate::database::models::{JobDbModel, JobExecutionProgressDbModel, LiveSessionDbModel};
use crate::database::repositories::{JobRepository, SessionRepository, SqlxJobRepository};
use crate::pipeline::{JobProgressSnapshot, ProgressKind};

use super::ServiceContainer;

async fn application() -> (ServiceContainer, Router, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::new(pool.clone(), pool).await.unwrap();
    let (logging, _layer) =
        crate::logging::LoggingConfig::for_route_tests(directory.path().to_path_buf());
    assert!(container.logging_config.set(Arc::new(logging)).is_ok());
    let mut state = container.build_api_state(None).unwrap();
    state.web_push_service = None;
    let application = crate::api::routes::create_router(state).layer(Extension(Claims {
        sub: "contract-user".to_owned(),
        roles: vec!["admin".to_owned()],
        iss: "test".to_owned(),
        aud: "test".to_owned(),
        exp: u64::MAX,
        iat: 0,
        sid: None,
    }));
    (container, application, directory)
}

async fn request(
    application: &Router,
    method: Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response =
        tokio::time::timeout(Duration::from_secs(5), application.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "{path}: {status}, invalid JSON {error}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, body)
}

async fn created(
    application: &Router,
    path: &str,
    documented_path: &str,
    body: Value,
    exercised: &mut BTreeSet<String>,
) -> Value {
    let (status, body) = request(application, Method::POST, path, body).await;
    assert_eq!(status, StatusCode::CREATED, "{path}: {body}");
    assert!(
        body.is_object(),
        "resource bodies must not become tuple arrays"
    );
    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let schema = &document["paths"][documented_path]["post"]["responses"]["201"]["content"]["application/json"]
        ["schema"];
    let reference = schema["$ref"].as_str().expect("resource schema reference");
    let schema = document
        .pointer(reference.strip_prefix('#').unwrap())
        .unwrap();
    for property in schema["required"].as_array().into_iter().flatten() {
        assert!(
            body.get(property.as_str().unwrap()).is_some(),
            "{path} omitted required property {property}"
        );
    }
    exercised.insert(documented_path.to_owned());
    body
}

#[tokio::test]
async fn every_documented_creation_returns_201_and_its_resource_body() {
    let (container, application, directory) = application().await;
    let mut exercised = BTreeSet::new();
    let streamer = created(&application, "/api/streamers", "/api/streamers", json!({
        "name": "contract-streamer", "url": "https://www.twitch.tv/contract_test", "enabled": false
    }), &mut exercised).await;
    let streamer_id = streamer["id"].as_str().unwrap();
    assert_eq!(streamer["name"], "contract-streamer");

    let template = created(
        &application,
        "/api/templates",
        "/api/templates",
        json!({"name": "contract-template"}),
        &mut exercised,
    )
    .await;
    let clone = created(
        &application,
        &format!("/api/templates/{}/clone", template["id"].as_str().unwrap()),
        "/api/templates/{id}/clone",
        json!({"new_name": "contract-template-copy"}),
        &mut exercised,
    )
    .await;
    assert_ne!(clone["id"], template["id"]);
    assert_eq!(clone["name"], "contract-template-copy");

    let engine = created(
        &application,
        "/api/engines",
        "/api/engines",
        json!({"name": "contract-engine", "engine_type": "MESIO", "config": {}}),
        &mut exercised,
    )
    .await;
    assert_eq!(engine["engine_type"], "MESIO");
    let filter = created(&application, &format!("/api/streamers/{streamer_id}/filters"), "/api/streamers/{streamer_id}/filters", json!({"streamer_id": streamer_id, "filter_type": "KEYWORD", "config": {"include": ["contract"], "exclude": []}}), &mut exercised).await;
    assert_eq!(filter["streamer_id"], streamer_id);

    let preset = created(&application, "/api/job/presets", "/api/job/presets", json!({"id": "contract-preset", "name": "contract-job", "processor": "execute", "config": "{}"}), &mut exercised).await;
    let clone = created(
        &application,
        "/api/job/presets/contract-preset/clone",
        "/api/job/presets/{id}/clone",
        json!({"new_name": "contract-job-copy"}),
        &mut exercised,
    )
    .await;
    assert_ne!(clone["id"], preset["id"]);
    assert_eq!(clone["name"], "contract-job-copy");

    let channel = created(&application, "/api/notifications/channels", "/api/notifications/channels", json!({"name": "contract-channel", "channel_type": "Gotify", "settings": {"server_url": "https://example.com", "app_token": "test"}}), &mut exercised).await;
    assert_eq!(channel["name"], "contract-channel");

    let dag = json!({"name": "contract-dag", "steps": [{"id": "step", "step": {"type": "inline", "processor": "execute", "config": {}}}]});
    let preset = created(
        &application,
        "/api/pipeline/presets",
        "/api/pipeline/presets",
        json!({"name": "contract-pipeline", "dag": dag}),
        &mut exercised,
    )
    .await;
    assert_eq!(preset["name"], "contract-pipeline");

    let proxy = created(
        &application,
        "/api/proxies",
        "/api/proxies",
        json!({"name": "contract-proxy", "url": "http://proxy.example:8080"}),
        &mut exercised,
    )
    .await;
    assert_eq!(proxy["url"], "http://proxy.example:8080");

    let session = LiveSessionDbModel::new(streamer_id);
    container
        .session_repository
        .create_session(&session)
        .await
        .unwrap();
    let input = directory.path().join("input.flv");
    tokio::fs::write(&input, b"test input").await.unwrap();
    let pipeline = created(&application, "/api/pipeline/create", "/api/pipeline/create", json!({"streamer_id": streamer_id, "session_id": session.id, "input_paths": [input], "dag": dag}), &mut exercised).await;
    assert!(pipeline["pipeline_id"].is_string());
    assert_eq!(pipeline["first_job"]["progress"], Value::Null);

    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let expected = document["paths"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, path)| path["post"]["responses"].get("201").is_some())
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        exercised, expected,
        "every documented creation needs an actual response assertion"
    );
    let (status, body) = request(
        &application,
        Method::POST,
        "/api/templates",
        json!({"name": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "VALIDATION_ERROR");
    container.cancellation_token.cancel();
}

/// Like [`request`], for responses that may have no body.
async fn send(
    application: &Router,
    method: Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        application.clone().oneshot(request),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn saved_proxies_hide_passwords_and_stay_while_routes_name_them() {
    const SECRET: &str = "proxy-password-sentinel";
    let (container, application, _directory) = application().await;
    let (status, proxy) = send(
        &application,
        Method::POST,
        "/api/proxies",
        json!({"name": "Office", "url": "HTTP://Proxy.Example:8080/", "username": "office-user", "password": SECRET}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{proxy}");
    let id = proxy["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            &proxy["url"],
            &proxy["scheme"],
            &proxy["host"],
            &proxy["port"]
        ),
        (
            &json!("http://proxy.example:8080"),
            &json!("http"),
            &json!("proxy.example"),
            &json!(8080)
        )
    );
    assert_eq!(proxy["username"], "office-user");
    assert_eq!(proxy["has_password"], true);
    let (status, duplicate) = send(
        &application,
        Method::POST,
        "/api/proxies",
        json!({"name": "office", "url": "http://other.example:1"}),
    )
    .await;
    assert_eq!(
        (status, &duplicate["code"]),
        (StatusCode::CONFLICT, &json!("PROXY_NAME_TAKEN"))
    );
    let (status, invalid) = send(
        &application,
        Method::POST,
        "/api/proxies",
        json!({"name": "Bad", "url": "socks4://bad.example:1"}),
    )
    .await;
    assert_eq!(
        (status, &invalid["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("PROXY_INVALID"))
    );

    // An omitted password keeps the saved one.
    let (status, renamed) = send(
        &application,
        Method::PATCH,
        &format!("/api/proxies/{id}"),
        json!({"expected_version": proxy["version"], "name": "Office proxy"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(
        container
            .proxies
            .endpoint(&id)
            .await
            .unwrap()
            .password
            .as_deref(),
        Some(SECRET)
    );
    let (status, stale) = send(
        &application,
        Method::PATCH,
        &format!("/api/proxies/{id}"),
        json!({"expected_version": proxy["version"], "name": "Stale"}),
    )
    .await;
    assert_eq!(
        (status, &stale["code"]),
        (StatusCode::CONFLICT, &json!("PROXY_STALE_VERSION"))
    );

    let route = json!({"kind": "proxy", "id": id});
    let (status, _) = send(
        &application,
        Method::PATCH,
        "/api/config/global",
        json!({"proxy_route": route}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused) = send(
        &application,
        Method::PATCH,
        "/api/config/global",
        json!({"proxy_config": "{\"enabled\":true}"}),
    )
    .await;
    assert_eq!(
        (status, &refused["code"]),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            &json!("PROXY_CONFIG_REPLACED")
        )
    );
    let (status, refused) = send(
        &application,
        Method::POST,
        "/api/templates",
        json!({"name": "Cookie template", "cookies": "session=abc"}),
    )
    .await;
    assert_eq!(
        (status, &refused["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("COOKIES_REPLACED"))
    );
    let (status, streamer) = send(
        &application,
        Method::POST,
        "/api/streamers",
        json!({"name": "routed", "url": "https://www.twitch.tv/routed_proxy", "enabled": false,
               "streamer_specific_config": {"proxy_route": route}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{streamer}");
    assert_eq!(streamer["streamer_specific_config"]["proxy_route"], route);
    let (status, refused) = send(
        &application,
        Method::POST,
        "/api/streamers",
        json!({"name": "legacy", "url": "https://www.twitch.tv/legacy_proxy", "enabled": false,
               "streamer_specific_config": {"proxy_config": {"enabled": false}}}),
    )
    .await;
    assert_eq!(
        (status, &refused["code"]),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            &json!("PROXY_CONFIG_REPLACED")
        )
    );
    let (status, effective) = send(
        &application,
        Method::GET,
        "/api/proxies/effective?scope_type=platform&scope_id=platform-huya",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        effective,
        json!({"kind": "proxy", "proxy": {"id": id, "name": "Office proxy"}, "source": "global"})
    );
    let (status, effective) = send(
        &application,
        Method::GET,
        &format!(
            "/api/proxies/effective?scope_type=streamer&scope_id={}",
            streamer["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, &effective["source"]),
        (StatusCode::OK, &json!("streamer"))
    );

    let (status, refused) = send(
        &application,
        Method::DELETE,
        &format!("/api/proxies/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "PROXY_REFERENCED");
    assert_eq!(refused["details"]["references"]["global"], true);
    assert_eq!(
        refused["details"]["references"]["streamers"][0]["name"],
        "routed"
    );
    let (status, detail) = send(
        &application,
        Method::GET,
        &format!("/api/proxies/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["proxy"]["usage_count"], 2);

    // Backups carry the login and name the proxy; IDs stay local.
    let (status, backup) = send(
        &application,
        Method::GET,
        "/api/config/backup/export",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(backup["version"], "1.0.0");
    assert_eq!(backup["proxies"][0]["password"], SECRET);
    assert_eq!(
        backup["global_config"]["proxy_route"],
        json!({"kind": "proxy", "name": "Office proxy"})
    );
    assert!(backup["global_config"].get("proxy_config").is_none());
    assert!(!backup.to_string().contains(&id));

    // Every other response keeps the password out.
    for path in [
        "/api/proxies".to_owned(),
        format!("/api/proxies/{id}"),
        "/api/proxies/system".to_owned(),
        "/api/config/global".to_owned(),
    ] {
        let (status, body) = send(&application, Method::GET, &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(!body.to_string().contains(SECRET), "{path}");
    }
    let (status, unknown) = send(
        &application,
        Method::POST,
        "/api/proxies/test",
        json!({"proxy_id": id, "platform": "unknown"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    let (status, _) = send(
        &application,
        Method::POST,
        "/api/proxies/test",
        json!({"url": "http://proxy.example:8080", "target_url": "http://127.0.0.1/"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &application,
        Method::PATCH,
        "/api/config/global",
        json!({"proxy_route": {"kind": "direct"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &application,
        Method::DELETE,
        &format!("/api/streamers/{}", streamer["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert!(status.is_success(), "{status}");
    let (status, _) = send(
        &application,
        Method::DELETE,
        &format!("/api/proxies/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    container.cancellation_token.cancel();
}

#[tokio::test]
async fn an_imported_proxy_on_another_exit_drops_its_throttle_and_moves_pinned_accounts() {
    let (container, application, _directory) = application().await;
    let (status, proxy) = send(
        &application,
        Method::POST,
        "/api/proxies",
        json!({"name": "Exit", "url": "http://old-exit.example:8080"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{proxy}");
    let id = proxy["id"].as_str().unwrap().to_owned();
    let profile = container
        .credential_profiles
        .create(
            "platform-huya",
            "Pinned",
            true,
            &crate::credentials::CredentialMaterial {
                cookies: "account=pinned".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            &crate::proxies::ProxyRoute::Proxy { id: id.clone() },
        )
        .await
        .unwrap();
    container
        .credential_profiles
        .publish_health(
            &profile,
            crate::credentials::CredentialValidity::Valid,
            None,
            true,
        )
        .await
        .unwrap();
    let key = crate::proxies::RouteKey::Proxy { id: id.clone() };
    let route = container
        .credential_profiles
        .account_route(&profile, None)
        .await
        .unwrap();
    container
        .platform_admission
        .defer("platform-huya", &route, Duration::from_secs(600));
    assert!(
        container
            .platform_admission
            .backing_off("platform-huya", &key)
    );

    let (status, mut backup) = send(
        &application,
        Method::GET,
        "/api/config/backup/export",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    backup["proxies"][0]["url"] = json!("http://new-exit.example:8080");
    let (status, imported) = send(
        &application,
        Method::POST,
        "/api/config/backup/import",
        json!({"config": backup, "mode": "merge"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{imported}");

    assert!(
        !container
            .platform_admission
            .backing_off("platform-huya", &key)
    );
    let moved = container
        .credential_profiles
        .get(&profile.id)
        .await
        .unwrap();
    assert!(moved.revision > profile.revision);
    assert!(
        container
            .credential_profiles
            .health(&profile.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        container.proxies.endpoint(&id).await.unwrap().url,
        "http://new-exit.example:8080"
    );
    container.cancellation_token.cancel();
}

#[tokio::test]
async fn newly_registered_routes_respond_through_the_complete_router() {
    let (container, application, _directory) = application().await;
    let mut session = LiveSessionDbModel::new("unused");
    session.streamer_id = None;
    container
        .session_repository
        .create_session(&session)
        .await
        .unwrap();
    let (status, body) = request(
        &application,
        Method::GET,
        &format!("/api/sessions/{}/segments", session.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    for (method, path, body) in [
        (
            Method::GET,
            "/api/notifications/web-push/public-key",
            Value::Null,
        ),
        (
            Method::GET,
            "/api/notifications/web-push/subscriptions",
            Value::Null,
        ),
        (
            Method::POST,
            "/api/notifications/web-push/subscribe",
            json!({"subscription": {"endpoint": "https://example.com/push", "keys": {"p256dh": "test", "auth": "test"}}}),
        ),
        (
            Method::POST,
            "/api/notifications/web-push/unsubscribe",
            json!({"endpoint": "https://example.com/push"}),
        ),
    ] {
        let (status, body) = request(&application, method, path, body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {body}");
        assert!(body["message"].as_str().unwrap().contains("Web push"));
    }
    container.cancellation_token.cancel();
}

#[tokio::test]
async fn job_summaries_report_unavailable_progress_for_every_status_and_preserve_snapshots() {
    let (container, application, _directory) = application().await;
    let repo = SqlxJobRepository::new(container.pool.clone(), container.write_pool.clone());
    for status in ["PENDING", "PROCESSING", "COMPLETED", "FAILED", "CANCELLED"] {
        let mut job = JobDbModel::new("execute", "{}");
        job.id = format!("contract-{status}");
        job.status = status.to_owned();
        repo.create_job(&job).await.unwrap();
        let (http_status, body) = request(
            &application,
            Method::GET,
            &format!("/api/pipeline/jobs/{}", job.id),
            Value::Null,
        )
        .await;
        assert_eq!(http_status, StatusCode::OK);
        assert_eq!(body["status"], status);
        assert_eq!(body.get("progress"), Some(&Value::Null));
    }
    for path in ["/api/pipeline/jobs", "/api/pipeline/jobs/page"] {
        let (status, body) = request(&application, Method::GET, path, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["items"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        assert!(
            rows.iter()
                .all(|row| row.get("progress") == Some(&Value::Null))
        );
    }
    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
    snapshot.percent = Some(42.5);
    repo.upsert_job_execution_progress(&JobExecutionProgressDbModel {
        job_id: "contract-PROCESSING".to_owned(),
        kind: "ffmpeg".to_owned(),
        progress: serde_json::to_string(&snapshot).unwrap(),
        updated_at: snapshot.updated_at.timestamp_millis(),
    })
    .await
    .unwrap();
    let (status, body) = request(
        &application,
        Method::GET,
        "/api/pipeline/jobs/contract-PROCESSING/progress",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["percent"], 42.5);
    let (status, _) = request(
        &application,
        Method::GET,
        "/api/pipeline/jobs/contract-PENDING/progress",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    container.cancellation_token.cancel();
}
