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
use crate::database::models::{
    DagExecutionDbModel, DagPipelineDefinition, DagStep, DagStepExecutionDbModel, JobDbModel,
    LiveSessionDbModel, PipelineStep,
};
use crate::database::repositories::{
    DagRepository, JobRepository, SessionRepository, SqlxDagRepository, SqlxJobRepository,
};

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
async fn dag_steps_report_their_job_state_and_only_a_retry_that_will_run() {
    let (container, application, _directory) = application().await;
    let jobs = SqlxJobRepository::new(container.pool.clone(), container.write_pool.clone());
    let dags = SqlxDagRepository::new(container.pool.clone(), container.write_pool.clone());
    let definition = DagPipelineDefinition::new(
        "contract-steps",
        ["done", "queued", "retrying", "failed", "blocked"]
            .into_iter()
            .map(|name| DagStep::new(name, PipelineStep::preset("remux")))
            .collect(),
    );
    let mut dag = DagExecutionDbModel::new(&definition, None, None);
    dag.status = "PROCESSING".to_owned();
    dag.completed_steps = 1;
    dag.failed_steps = 1;
    dags.create_dag(&dag).await.unwrap();

    for (id, status, started_at) in [
        ("job-done", "COMPLETED", Some(1_000)),
        ("job-queued", "PENDING", None),
        ("job-retrying", "PROCESSING", Some(3_000)),
        ("job-failed", "FAILED", Some(4_000)),
    ] {
        let mut job = JobDbModel::new("remux", "{}");
        job.id = id.to_owned();
        job.status = status.to_owned();
        job.started_at = started_at;
        jobs.create_job(&job).await.unwrap();
    }
    let retry_after = crate::database::time::now_ms() + 3_600_000;
    assert_eq!(
        jobs.mark_job_failed("job-retrying", "boom", Some(retry_after))
            .await
            .unwrap(),
        1
    );
    let steps: Vec<_> = [
        ("done", "COMPLETED", Some("job-done")),
        ("queued", "PROCESSING", Some("job-queued")),
        ("retrying", "PROCESSING", Some("job-retrying")),
        ("failed", "FAILED", Some("job-failed")),
        ("blocked", "BLOCKED", None),
    ]
    .into_iter()
    .map(|(name, status, job_id)| {
        let mut step = DagStepExecutionDbModel::new(&dag.id, name, &[]);
        step.status = status.to_owned();
        step.job_id = job_id.map(str::to_owned);
        step.created_at = dag.created_at;
        step
    })
    .collect();
    dags.create_steps(&steps).await.unwrap();

    let path = format!("/api/pipeline/dag/{}", dag.id);
    let (status, body) = request(&application, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Steps that share created_at still come back in definition order.
    let order: Vec<_> = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step_id"].clone())
        .collect();
    assert_eq!(
        order,
        ["done", "queued", "retrying", "failed", "blocked"].map(|name| json!(name))
    );
    let step = |body: &Value, name: &str| {
        body["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["step_id"] == name)
            .unwrap()
            .clone()
    };
    for (name, job_status, started_at, retry) in [
        ("done", Some("COMPLETED"), Some(1_000), None),
        ("queued", Some("PENDING"), None, None),
        ("retrying", Some("FAILED"), Some(3_000), Some(retry_after)),
        ("failed", Some("FAILED"), Some(4_000), None),
        ("blocked", None, None, None),
    ] {
        let step = step(&body, name);
        assert_eq!(step.get("job_status"), Some(&json!(job_status)), "{name}");
        assert_eq!(step.get("started_at"), Some(&json!(started_at)), "{name}");
        assert_eq!(step.get("retry_after"), Some(&json!(retry)), "{name}");
        assert_eq!(step.get("progress"), Some(&Value::Null), "{name}");
    }

    // One of five steps completed and none is running; the failed step counts
    // nothing, on every endpoint.
    assert_eq!(body["progress_percent"], json!(20.0));
    let (status, list) =
        request(&application, Method::GET, "/api/pipeline/dags", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let item = list["dags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == dag.id.as_str())
        .unwrap();
    assert_eq!(item["progress_percent"], json!(20.0));
    let (status, stats) = request(
        &application,
        Method::GET,
        &format!("{path}/stats"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stats}");
    assert_eq!(stats["progress_percent"], json!(20.0));
    let (status, missing) = request(
        &application,
        Method::GET,
        "/api/pipeline/dag/does-not-exist/stats",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");

    let (status, body) = request(&application, Method::DELETE, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The cancelled workflow drops the retry while the job row still holds it.
    assert_eq!(
        jobs.get_job("job-retrying").await.unwrap().retry_after,
        Some(retry_after)
    );
    let (status, body) = request(&application, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let retrying = step(&body, "retrying");
    assert_eq!(retrying.get("job_status"), Some(&json!("FAILED")));
    assert_eq!(retrying.get("retry_after"), Some(&Value::Null));
    container.cancellation_token.cancel();
}
