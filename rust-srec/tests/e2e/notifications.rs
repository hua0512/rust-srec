//! Notification channels, subscriptions, the event catalog and log, and the
//! test button, driven over HTTP with a [`LocalHttpServer`] as every channel's
//! receiver.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    LocalHttpServer, ReceivedRequest, Response, TestApp, WAIT, assert_recent, eventually, id_of,
    millis, now_ms, sorted_keys,
};

/// `SystemStartup` rendered for the test button, whose event carries version `test`.
const EN_TITLE: &str = "🚀 System started (vtest)";
const EN_DESCRIPTION: &str = "System initialized successfully (vtest)";
const ZH_TITLE: &str = "🚀 系统已启动 (vtest)";
const ZH_DESCRIPTION: &str = "系统初始化成功 (vtest)";

async fn create_channel(
    app: &TestApp,
    token: &str,
    name: &str,
    channel_type: &str,
    settings: Value,
) -> String {
    let response = app
        .post(
            "/api/notifications/channels",
            token,
            &json!({ "name": name, "channel_type": channel_type, "settings": settings }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    response.body["id"]
        .as_str()
        .expect("channel id in the create response")
        .to_owned()
}

async fn test_send(app: &TestApp, token: &str, id: &str) -> Response {
    app.send(
        Method::POST,
        &format!("/api/notifications/channels/{id}/test"),
        Some(token),
        None,
    )
    .await
}

async fn loaded_channel_ids(app: &TestApp, token: &str) -> Vec<String> {
    let response = app
        .get("/api/notifications/instances", token)
        .await
        .expect_status(StatusCode::OK);
    response
        .body
        .as_array()
        .expect("instances array")
        .iter()
        .filter_map(|instance| instance["channel_id"].as_str().map(str::to_owned))
        .collect()
}

fn json_body(request: &ReceivedRequest) -> Value {
    serde_json::from_slice(&request.body).expect("JSON request body")
}

fn event_ids(body: &Value) -> Vec<&str> {
    body.as_array()
        .expect("event array")
        .iter()
        .map(|event| event["id"].as_str().expect("event id"))
        .collect()
}

#[tokio::test]
async fn webhook_test_send_delivers_the_documented_payload_headers_and_each_auth_kind() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "ok").await;
    let url = receiver.url("/hook");

    let id = create_channel(
        &app,
        &token,
        "Ops webhook",
        "Webhook",
        json!({
            "url": url,
            "method": "POST",
            "headers": { "X-A": "1" },
            "auth": { "type": "Bearer", "token": "bearer-secret" },
            "locale": "en",
        }),
    )
    .await;
    test_send(&app, &token, &id)
        .await
        .expect_status(StatusCode::OK);

    let received = receiver.received();
    assert_eq!(received.len(), 1, "the test send is delivered exactly once");
    let request = &received[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/hook");
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.header("authorization"),
        Some("Bearer bearer-secret")
    );
    assert_eq!(request.header("x-a"), Some("1"));
    let body = json_body(request);
    assert_eq!(
        sorted_keys(&body),
        [
            "data",
            "description",
            "event_type",
            "priority",
            "priority_label",
            "streamer_id",
            "timestamp",
            "title"
        ]
    );
    assert_eq!(body["event_type"], "system_startup");
    assert_eq!(body["priority"], 5);
    assert_eq!(body["priority_label"], "normal");
    assert_eq!(body["title"], EN_TITLE);
    assert_eq!(body["description"], EN_DESCRIPTION);
    assert_eq!(body["streamer_id"], Value::Null);
    assert_recent(millis(&body["timestamp"]));
    assert_eq!(body["data"]["SystemStartup"]["version"], "test");
    assert_recent(millis(&body["data"]["SystemStartup"]["timestamp"]));

    // Basic auth over PUT; the edit replaces the settings, so the custom
    // header is gone too.
    app.put(
        &format!("/api/notifications/channels/{id}"),
        &token,
        &json!({
            "name": "Ops webhook",
            "settings": {
                "url": url,
                "method": "PUT",
                "auth": { "type": "Basic", "username": "user", "password": "pass" },
                "locale": "en",
            },
        }),
    )
    .await
    .expect_status(StatusCode::OK);
    test_send(&app, &token, &id)
        .await
        .expect_status(StatusCode::OK);
    let received = receiver.received();
    assert_eq!(received.len(), 2);
    let request = &received[1];
    assert_eq!(request.method, "PUT");
    assert_eq!(request.header("authorization"), Some("Basic dXNlcjpwYXNz"));
    assert_eq!(request.header("x-a"), None);
    assert_eq!(json_body(request)["event_type"], "system_startup");

    // A custom auth header, with the method left to its POST default.
    app.put(
        &format!("/api/notifications/channels/{id}"),
        &token,
        &json!({
            "name": "Ops webhook",
            "settings": {
                "url": url,
                "auth": { "type": "Header", "name": "X-Token", "value": "header-secret" },
            },
        }),
    )
    .await
    .expect_status(StatusCode::OK);
    test_send(&app, &token, &id)
        .await
        .expect_status(StatusCode::OK);
    let received = receiver.received();
    assert_eq!(received.len(), 3);
    let request = &received[2];
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("x-token"), Some("header-secret"));
    assert_eq!(request.header("authorization"), None);

    app.shutdown().await;
}

#[tokio::test]
async fn discord_and_gotify_test_sends_reach_their_documented_endpoints() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "{}").await;

    let discord = create_channel(
        &app,
        &token,
        "Discord",
        "Discord",
        json!({ "webhook_url": receiver.url("/discord/hook"), "locale": "en" }),
    )
    .await;
    test_send(&app, &token, &discord)
        .await
        .expect_status(StatusCode::OK);
    let received = receiver.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].method, "POST");
    assert_eq!(received[0].path, "/discord/hook");
    let body = json_body(&received[0]);
    assert_eq!(sorted_keys(&body), ["embeds"], "no username or avatar set");
    let embeds = body["embeds"].as_array().expect("embeds array");
    assert_eq!(embeds.len(), 1);
    let embed = &embeds[0];
    assert_eq!(embed["title"], EN_TITLE);
    assert_eq!(embed["description"], EN_DESCRIPTION);
    assert_recent(millis(&embed["timestamp"]));

    let gotify = create_channel(
        &app,
        &token,
        "Gotify",
        "Gotify",
        json!({
            "server_url": receiver.url("/gotify/"),
            "app_token": "gotify-token",
            "locale": "en",
        }),
    )
    .await;
    test_send(&app, &token, &gotify)
        .await
        .expect_status(StatusCode::OK);
    let received = receiver.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[1].method, "POST");
    assert_eq!(received[1].path, "/gotify/message?token=gotify-token");
    assert_eq!(
        json_body(&received[1]),
        json!({ "title": EN_TITLE, "message": EN_DESCRIPTION, "priority": 5 })
    );

    app.shutdown().await;
}

/// The bodies `channel-form.tsx` submits for each type, with its defaults:
/// integer `min_priority`, blank `locale`, explicit `enabled`. A webhook
/// stored with a string `min_priority` label loads too.
#[tokio::test]
async fn channels_saved_in_the_form_shape_load_send_and_survive_a_backup() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "{}").await;

    let discord = create_channel(
        &app,
        &token,
        "Form Discord",
        "Discord",
        json!({
            "webhook_url": receiver.url("/discord"),
            "min_priority": 5,
            "locale": "",
            "enabled": true,
        }),
    )
    .await;
    let gotify = create_channel(
        &app,
        &token,
        "Form Gotify",
        "Gotify",
        json!({
            "server_url": receiver.url(""),
            "app_token": "form-token",
            "min_priority": 5,
            "locale": "",
            "enabled": true,
            "timeout_secs": 30,
        }),
    )
    .await;
    let telegram = create_channel(
        &app,
        &token,
        "Form Telegram",
        "Telegram",
        json!({
            "bot_token": "123:form-bot-token",
            "chat_id": "42",
            "parse_mode": "HTML",
            "min_priority": 5,
            "locale": "",
            "enabled": true,
        }),
    )
    .await;
    let email = create_channel(
        &app,
        &token,
        "Form Email",
        "Email",
        json!({
            "smtp_host": "127.0.0.1",
            "smtp_port": 587,
            "username": "",
            "password": "",
            "from_address": "srec@example.com",
            "to_addresses": ["ops@example.com"],
            "use_tls": true,
            "min_priority": 8,
            "locale": "",
            "enabled": true,
        }),
    )
    .await;
    // `channel-form.tsx` turns auth `None` into null, and `removeEmpty` drops it.
    let webhook = create_channel(
        &app,
        &token,
        "Form webhook",
        "Webhook",
        json!({
            "url": receiver.url("/form"),
            "method": "POST",
            "min_priority": 2,
            "locale": "",
            "enabled": true,
            "timeout_secs": 30,
            "headers": {},
        }),
    )
    .await;
    let labelled = create_channel(
        &app,
        &token,
        "Labelled webhook",
        "Webhook",
        json!({ "url": receiver.url("/labelled"), "min_priority": "low" }),
    )
    .await;

    let instances = app
        .get("/api/notifications/instances", &token)
        .await
        .expect_status(StatusCode::OK);
    let instances = instances.body.as_array().expect("instances array");
    for (id, name, channel_type) in [
        (&discord, "Form Discord", "Discord"),
        (&gotify, "Form Gotify", "Gotify"),
        (&telegram, "Form Telegram", "Telegram"),
        (&email, "Form Email", "Email"),
        (&webhook, "Form webhook", "Webhook"),
        (&labelled, "Labelled webhook", "Webhook"),
    ] {
        let instance = instances
            .iter()
            .find(|instance| instance["channel_id"] == id.as_str())
            .unwrap_or_else(|| panic!("{channel_type} channel is loaded: {instances:?}"));
        assert_eq!(instance["key"], id.as_str());
        assert_eq!(instance["display_name"], name);
        assert_eq!(instance["channel_type"], channel_type);
        assert_eq!(instance["source"], "database");
    }

    // Telegram and Email would leave the machine, so they don't send.
    for id in [&discord, &gotify, &webhook, &labelled] {
        test_send(&app, &token, id)
            .await
            .expect_status(StatusCode::OK);
    }
    let paths: Vec<String> = receiver
        .received()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(
        paths,
        [
            "/discord",
            "/message?token=form-token",
            "/form",
            "/labelled"
        ]
    );

    // A backup holding them imports again.
    let bundle = app
        .get("/api/config/backup/export", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    let imported = app
        .post(
            "/api/config/backup/import",
            &token,
            &json!({ "config": bundle, "mode": "merge" }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(imported.body["stats"]["channels_updated"], 6);

    app.shutdown().await;
}

/// `GET` returns the stored settings verbatim, secrets included, on purpose:
/// the edit form re-fills every field from it.
#[tokio::test]
async fn channel_crud_returns_stored_settings_for_the_edit_form_and_delete_removes_subscriptions() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let settings = json!({
        "url": "http://127.0.0.1:9/hook",
        "headers": { "X-Api-Key": "header-secret" },
        "auth": { "type": "Bearer", "token": "crud-secret-token" },
    });
    let created = app
        .post(
            "/api/notifications/channels",
            &token,
            &json!({ "name": "CRUD webhook", "channel_type": "Webhook", "settings": settings }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    assert_eq!(
        sorted_keys(&created.body),
        ["channel_type", "id", "name", "settings"]
    );
    let id = created.body["id"].as_str().expect("channel id").to_owned();
    assert_eq!(id.len(), 36, "the id is a UUID: {id}");
    assert_eq!(created.body["name"], "CRUD webhook");
    assert_eq!(created.body["channel_type"], "Webhook");
    let stored = created.body["settings"]
        .as_str()
        .expect("settings come back as a JSON string");
    assert_eq!(
        serde_json::from_str::<Value>(stored).expect("settings JSON"),
        settings
    );

    let rejected = app
        .post(
            "/api/notifications/channels",
            &token,
            &json!({ "name": "Slack", "channel_type": "Slack", "settings": {} }),
        )
        .await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);

    let listed = app
        .get("/api/notifications/channels", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        listed.body,
        json!([created.body]),
        "the rejected type wrote nothing"
    );
    let fetched = app
        .get(&format!("/api/notifications/channels/{id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(fetched.body, created.body);
    assert!(fetched.text.contains("crud-secret-token"));
    assert!(fetched.text.contains("header-secret"));

    let missing = app
        .put(
            "/api/notifications/channels/00000000-0000-0000-0000-000000000000",
            &token,
            &json!({ "name": "Ghost", "settings": settings }),
        )
        .await
        .expect_status(StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), Some("NOT_FOUND"));

    let renamed_settings = json!({
        "url": "http://127.0.0.1:9/renamed",
        "auth": { "type": "Basic", "username": "ops", "password": "basic-secret" },
    });
    let updated = app
        .put(
            &format!("/api/notifications/channels/{id}"),
            &token,
            &json!({ "name": "Renamed webhook", "settings": renamed_settings }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(updated.body["id"], id.as_str());
    assert_eq!(updated.body["channel_type"], "Webhook");
    let fetched = app
        .get(&format!("/api/notifications/channels/{id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(fetched.body["name"], "Renamed webhook");
    assert_eq!(
        serde_json::from_str::<Value>(fetched.body["settings"].as_str().expect("settings"))
            .expect("settings JSON"),
        renamed_settings
    );

    app.put(
        &format!("/api/notifications/channels/{id}/subscriptions"),
        &token,
        &json!({ "events": ["stream_online", "fatal_error"] }),
    )
    .await
    .expect_status(StatusCode::OK);
    assert!(loaded_channel_ids(&app, &token).await.contains(&id));

    let deleted = app
        .delete(&format!("/api/notifications/channels/{id}"), &token)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let gone = app
        .get(&format!("/api/notifications/channels/{id}"), &token)
        .await
        .expect_status(StatusCode::NOT_FOUND);
    assert_eq!(gone.error_code(), Some("NOT_FOUND"));
    let listed = app
        .get("/api/notifications/channels", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(listed.body, json!([]));
    assert!(!loaded_channel_ids(&app, &token).await.contains(&id));
    let subscriptions = app
        .get(
            &format!("/api/notifications/channels/{id}/subscriptions"),
            &token,
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(subscriptions.body, json!([]));
    let database = app.database().await;
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notification_subscription WHERE channel_id = ?")
            .bind(&id)
            .fetch_one(&database)
            .await
            .expect("count subscriptions");
    assert_eq!(rows, 0, "deleting a channel removes its subscriptions");
    database.close().await;

    app.shutdown().await;
}

/// The read-only allowlist names the event catalog and log exactly; the
/// channel routes beside them stay refused.
#[tokio::test]
async fn a_read_only_api_key_reads_the_event_catalog_and_log_but_not_channels() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let id = create_channel(
        &app,
        &token,
        "Guarded",
        "Webhook",
        json!({ "url": "http://127.0.0.1:9/guarded" }),
    )
    .await;
    let key = app.create_api_key(&token, "reader", "read_only").await.raw;

    for path in [
        "/api/notifications/channels".to_owned(),
        format!("/api/notifications/channels/{id}"),
        "/api/notifications/instances".to_owned(),
    ] {
        let response = app.get(&path, &key).await;
        assert_eq!(
            response.status,
            StatusCode::FORBIDDEN,
            "{path}: {}",
            response.text
        );
        assert_eq!(response.error_code(), Some("API_KEY_READ_ONLY"));
    }

    let catalog = app
        .get("/api/notifications/event-types", &key)
        .await
        .expect_status(StatusCode::OK);
    assert!(!catalog.body.as_array().expect("catalog array").is_empty());
    let events = app
        .get("/api/notifications/events", &key)
        .await
        .expect_status(StatusCode::OK);
    assert!(events.body.is_array());

    app.shutdown().await;
}

#[tokio::test]
async fn test_send_reports_disabled_and_failing_channels_without_leaking_the_receiver() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "ok").await;

    let disabled = create_channel(
        &app,
        &token,
        "Paused",
        "Webhook",
        json!({ "url": receiver.url("/paused"), "enabled": false }),
    )
    .await;
    assert!(!loaded_channel_ids(&app, &token).await.contains(&disabled));
    let response = test_send(&app, &token, &disabled)
        .await
        .expect_status(StatusCode::NOT_FOUND);
    assert_eq!(response.error_code(), Some("NOT_FOUND"));
    assert!(receiver.received().is_empty());

    let failing_receiver = LocalHttpServer::start(500, "receiver-private-body").await;
    let failing_url = failing_receiver.url("/private-hook-path?token=query-secret");
    let failing = create_channel(
        &app,
        &token,
        "Failing",
        "Webhook",
        json!({ "url": failing_url }),
    )
    .await;
    let response = test_send(&app, &token, &failing)
        .await
        .expect_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.error_code(), Some("INTERNAL_ERROR"));
    let authority = failing_url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .expect("receiver authority");
    for leaked in [
        "private-hook-path",
        "query-secret",
        "receiver-private-body",
        authority,
    ] {
        assert!(
            !response.text.contains(leaked),
            "the error echoes {leaked}: {}",
            response.text
        );
    }
    assert_eq!(
        failing_receiver.received().len(),
        1,
        "the test send is attempted once"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn email_test_rejects_a_half_configured_smtp_login_or_a_blank_host_before_connecting() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    // A plain listener: any SMTP connection attempt would sit in its backlog.
    let smtp = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the SMTP listener");
    smtp.set_nonblocking(true)
        .expect("make the SMTP listener non-blocking");
    let port = smtp.local_addr().expect("SMTP listener address").port();
    let email = |host: &str, username: &str| {
        json!({
            "smtp_host": host,
            "smtp_port": port,
            "username": username,
            "password": "",
            "from_address": "srec@example.com",
            "to_addresses": ["ops@example.com"],
            "use_tls": false,
            "min_priority": 5,
        })
    };

    let half = create_channel(
        &app,
        &token,
        "Half login",
        "Email",
        email("127.0.0.1", "mailer"),
    )
    .await;
    test_send(&app, &token, &half)
        .await
        .assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    let blank = create_channel(&app, &token, "Blank host", "Email", email("   ", "")).await;
    test_send(&app, &token, &blank)
        .await
        .assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    match smtp.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the SMTP server was contacted: {other:?}"),
    }

    app.shutdown().await;
}

#[tokio::test]
#[ignore = "bug: the test button returns 200 without sending when min_priority filters out the Normal-priority test event"]
async fn known_bug_test_send_does_not_report_success_when_min_priority_filters_it() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "{}").await;
    // The email form's default threshold; Discord shares the same rule.
    let id = create_channel(
        &app,
        &token,
        "High only",
        "Discord",
        json!({ "webhook_url": receiver.url("/high"), "min_priority": 8 }),
    )
    .await;

    let response = test_send(&app, &token, &id).await;
    assert!(
        response.status != StatusCode::OK || receiver.received().len() == 1,
        "the test reported success ({}) but nothing was sent",
        response.status
    );

    app.shutdown().await;
}

#[tokio::test]
async fn the_channel_locale_decides_the_language_of_the_test_payload() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "ok").await;

    let cases = [
        ("/zh", json!("zh-CN")),
        ("/en", json!("en")),
        ("/padded", json!("  zh-CN  ")),
        ("/unknown", json!("xx-YY")),
        ("/blank", json!("")),
        ("/absent", Value::Null),
    ];
    for (path, locale) in &cases {
        let mut settings = json!({ "url": receiver.url(path) });
        if !locale.is_null() {
            settings["locale"] = locale.clone();
        }
        let id = create_channel(&app, &token, path, "Webhook", settings).await;
        test_send(&app, &token, &id)
            .await
            .expect_status(StatusCode::OK);
    }

    let received = receiver.received();
    let payload = |path: &str| {
        let request = received
            .iter()
            .find(|request| request.path == path)
            .unwrap_or_else(|| panic!("{path} received the test send"));
        json_body(request)
    };
    assert_eq!(payload("/zh")["title"], ZH_TITLE);
    assert_eq!(payload("/zh")["description"], ZH_DESCRIPTION);
    assert_eq!(payload("/en")["title"], EN_TITLE);
    assert_eq!(payload("/en")["description"], EN_DESCRIPTION);
    assert_eq!(
        payload("/padded")["title"],
        ZH_TITLE,
        "the locale is trimmed"
    );
    assert_eq!(
        payload("/unknown")["title"],
        EN_TITLE,
        "unknown falls back to English"
    );
    // Blank and absent both follow the process locale, whatever it is.
    let process = payload("/absent")["title"].clone();
    assert!(process == EN_TITLE || process == ZH_TITLE, "{process}");
    assert_eq!(payload("/blank")["title"], process);

    app.shutdown().await;
}

#[tokio::test]
async fn subscriptions_echo_the_request_and_read_back_sorted_under_canonical_names() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let id = create_channel(
        &app,
        &token,
        "Subscribed",
        "Webhook",
        json!({ "url": "http://127.0.0.1:9/subscribed" }),
    )
    .await;
    let path = format!("/api/notifications/channels/{id}/subscriptions");

    let requested = json!(["stream_online", "StreamOffline", "download.error"]);
    let saved = app
        .put(&path, &token, &json!({ "events": requested }))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(saved.body, requested, "the response echoes the request");
    let stored = app.get(&path, &token).await.expect_status(StatusCode::OK);
    assert_eq!(
        stored.body,
        json!(["download_error", "stream_offline", "stream_online"]),
        "legacy aliases are rewritten to canonical names on reload"
    );

    app.put(&path, &token, &json!({ "events": ["stream_online"] }))
        .await
        .expect_status(StatusCode::OK);
    let stored = app.get(&path, &token).await.expect_status(StatusCode::OK);
    assert_eq!(stored.body, json!(["stream_online"]));

    let unknown = "/api/notifications/channels/00000000-0000-0000-0000-000000000000/subscriptions";
    let response = app
        .put(unknown, &token, &json!({ "events": ["stream_online"] }))
        .await
        .expect_status(StatusCode::NOT_FOUND);
    assert_eq!(response.error_code(), Some("NOT_FOUND"));
    let response = app.get(unknown, &token).await.expect_status(StatusCode::OK);
    assert_eq!(response.body, json!([]));

    app.shutdown().await;
}

#[tokio::test]
async fn the_event_type_catalog_lists_canonical_types_with_integer_priorities() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let catalog = app
        .get("/api/notifications/event-types", &token)
        .await
        .expect_status(StatusCode::OK);
    let entries = catalog.body.as_array().expect("catalog array");
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        assert_eq!(
            sorted_keys(entry),
            ["aliases", "event_type", "label", "priority"]
        );
        let event_type = entry["event_type"].as_str().expect("event_type string");
        assert!(seen.insert(event_type), "{event_type} is listed once");
        assert!(
            event_type
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'),
            "{event_type} is snake_case"
        );
        assert!(!entry["label"].as_str().expect("label").is_empty());
        let priority = entry["priority"].as_u64().expect("integer priority");
        assert!(
            [2, 5, 8, 10].contains(&priority),
            "{event_type}: {priority}"
        );
        let aliases: Vec<&str> = entry["aliases"]
            .as_array()
            .expect("aliases array")
            .iter()
            .map(|alias| alias.as_str().expect("alias string"))
            .collect();
        assert!(aliases.contains(&event_type));
    }
    let entry = |event_type: &str| {
        entries
            .iter()
            .find(|entry| entry["event_type"] == event_type)
            .unwrap_or_else(|| panic!("{event_type} is offered"))
    };
    assert_eq!(entry("stream_online")["label"], "Stream Online");
    assert_eq!(entry("stream_online")["priority"], 5);
    assert!(
        entry("stream_online")["aliases"]
            .as_array()
            .expect("aliases")
            .contains(&json!("StreamOnline"))
    );
    assert_eq!(entry("stream_offline")["priority"], 2);
    assert_eq!(entry("download_error")["priority"], 8);
    assert_eq!(entry("fatal_error")["priority"], 10);
    assert_eq!(entry("credential_unavailable")["priority"], 8);

    app.shutdown().await;
}

#[tokio::test]
#[ignore = "bug: config_updated is offered for subscription, but nothing emits it any more"]
async fn known_bug_event_type_catalog_offers_only_emitted_events() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let catalog = app
        .get("/api/notifications/event-types", &token)
        .await
        .expect_status(StatusCode::OK);
    let offered: Vec<&str> = catalog
        .body
        .as_array()
        .expect("catalog array")
        .iter()
        .filter_map(|entry| entry["event_type"].as_str())
        .collect();
    assert!(!offered.contains(&"config_updated"), "{offered:?}");

    app.shutdown().await;
}

#[tokio::test]
async fn the_event_log_lists_newest_first_and_filters_and_pages() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let alpha = id_of(
        &app.create_disabled_streamer(
            &token,
            json!({ "name": "Alpha Caster", "url": "https://www.huya.com/880001" }),
        )
        .await,
    );
    let beta = id_of(
        &app.create_disabled_streamer(
            &token,
            json!({ "name": "Beta Caster", "url": "https://www.huya.com/880002" }),
        )
        .await,
    );

    // Nothing offline emits these, so seed the log the handler reads.
    let now = now_ms();
    let rows = [
        (
            "00000000-0000-4000-8000-000000000001",
            "stream_online",
            5,
            r#"{"StreamOnline":{"title":"Morning show"}}"#,
            Some(alpha.as_str()),
            now - 4000,
        ),
        (
            "00000000-0000-4000-8000-000000000002",
            "download_error",
            8,
            r#"{"DownloadError":{"error_message":"Disk Quota exceeded"}}"#,
            Some(beta.as_str()),
            now - 3000,
        ),
        (
            "00000000-0000-4000-8000-000000000003",
            "fatal_error",
            10,
            r#"{"FatalError":{"error_type":"auth"}}"#,
            Some(alpha.as_str()),
            now - 2000,
        ),
        (
            "00000000-0000-4000-8000-000000000004",
            "system_startup",
            5,
            r#"{"SystemStartup":{"version":"1.0.0"}}"#,
            None,
            now - 1000,
        ),
    ];
    let database = app.database().await;
    for (id, event_type, priority, payload, streamer_id, created_at) in rows {
        sqlx::query(
            "INSERT INTO notification_event_log (id, event_type, priority, payload, streamer_id, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(event_type)
        .bind(priority)
        .bind(payload)
        .bind(streamer_id)
        .bind(created_at)
        .execute(&database)
        .await
        .expect("seed the event log");
    }
    // Older, low-priority filler that no filter below selects, for the page bounds.
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1001)
         INSERT INTO notification_event_log (id, event_type, priority, payload, streamer_id, created_at)
         SELECT printf('10000000-0000-4000-8000-%012d', i), 'stream_offline', 2, '{}', NULL, ? - i FROM n",
    )
    .bind(now - 10_000)
    .execute(&database)
    .await
    .expect("seed older events");
    database.close().await;
    let [e1, e2, e3, e4] = rows.map(|row| row.0);

    let all = app
        .get("/api/notifications/events", &token)
        .await
        .expect_status(StatusCode::OK);
    let ids = event_ids(&all.body);
    assert_eq!(ids.len(), 200, "the default page holds 200 events");
    assert_eq!(ids[..4], [e4, e3, e2, e1], "newest first");
    let entries = all.body.as_array().expect("events");
    assert_eq!(
        sorted_keys(&entries[1]),
        [
            "created_at",
            "event_type",
            "id",
            "payload",
            "priority",
            "streamer_id"
        ]
    );
    assert_eq!(entries[1]["event_type"], "fatal_error");
    assert_eq!(entries[1]["priority"], 10);
    assert_eq!(entries[1]["streamer_id"], alpha.as_str());
    assert_eq!(entries[1]["created_at"], now - 2000);
    assert_eq!(
        entries[1]["payload"], rows[2].3,
        "the payload stays a string"
    );
    assert!(
        entries[0].get("streamer_id").is_none(),
        "no streamer, no key"
    );

    let by_alpha = format!("streamer_id={alpha}");
    let fatal_for_beta = format!("event_type=fatal_error&streamer_id={beta}");
    let queries: [(&str, Vec<&str>); 13] = [
        ("event_type=stream_online", vec![e1]),
        (&by_alpha, vec![e3, e1]),
        (&fatal_for_beta, vec![]),
        ("priority=high", vec![e3, e2]),
        ("priority=8", vec![e3, e2]),
        ("priority=critical", vec![e3]),
        // Payload text and the streamer's name, ASCII case-insensitive.
        ("search=disk%20quota", vec![e2]),
        ("search=BETA%20caster", vec![e2]),
        // `%` is literal, not a wildcard.
        ("search=%25", vec![]),
        ("limit=2", vec![e4, e3]),
        ("limit=2&offset=2", vec![e2, e1]),
        ("limit=0", vec![e4]),
        ("offset=-5&limit=1", vec![e4]),
    ];
    let largest = app
        .get("/api/notifications/events?limit=5000", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        largest.body.as_array().map(Vec::len),
        Some(1000),
        "the limit is capped at 1000"
    );
    for (query, expected) in queries {
        let response = app
            .get(&format!("/api/notifications/events?{query}"), &token)
            .await
            .expect_status(StatusCode::OK);
        assert_eq!(event_ids(&response.body), expected, "?{query}");
    }

    app.shutdown().await;
}

async fn credential_unavailable_log(app: &TestApp, token: &str) -> Vec<Value> {
    app.get(
        "/api/notifications/events?event_type=credential_unavailable",
        token,
    )
    .await
    .expect_status(StatusCode::OK)
    .body
    .as_array()
    .expect("events")
    .clone()
}

#[tokio::test]
async fn a_credential_unavailable_event_reaches_only_subscribed_channels_and_is_logged_once() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let subscribed = LocalHttpServer::start(200, "ok").await;
    let unsubscribed = LocalHttpServer::start(200, "ok").await;
    for (name, receiver, events) in [
        ("Subscribed", &subscribed, json!(["credential_unavailable"])),
        ("Unsubscribed", &unsubscribed, json!(["stream_online"])),
    ] {
        let id = create_channel(
            &app,
            &token,
            name,
            "Webhook",
            json!({ "url": receiver.url("/event"), "locale": "en" }),
        )
        .await;
        app.put(
            &format!("/api/notifications/channels/{id}/subscriptions"),
            &token,
            &json!({ "events": events }),
        )
        .await
        .expect_status(StatusCode::OK);
    }
    app.select_disabled_account(&token, "platform-huya").await;
    app.select_disabled_account(&token, "platform-douyin").await;

    let parse = |url: &'static str| {
        let app = &app;
        let token = &token;
        async move {
            let response = app
                .post("/api/parse", token, &json!({ "url": url }))
                .await
                .expect_status(StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.error_code(), Some("CREDENTIAL_UNAVAILABLE"));
            assert_eq!(response.body["details"]["reason"], "profiles_disabled");
        }
    };
    parse("https://www.huya.com/770001").await;

    let delivered = eventually(
        WAIT,
        "the subscribed channel receives the event",
        || async {
            let received = subscribed.received();
            (!received.is_empty()).then_some(received)
        },
    )
    .await;
    let body = json_body(&delivered[0]);
    assert_eq!(body["event_type"], "credential_unavailable");
    assert_eq!(body["priority"], 8);
    assert_eq!(body["priority_label"], "high");
    assert!(body["data"].get("Credential").is_some(), "{body}");
    assert!(body.to_string().contains("huya"), "{body}");
    let logged = eventually(WAIT, "the event log records the event", || async {
        let log = credential_unavailable_log(&app, &token).await;
        (!log.is_empty()).then_some(log)
    })
    .await;
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["priority"], 8);

    // The same exhausted selection is not announced again for ten minutes.
    // Douyin's first failure, announced after it, shows the repeat was not.
    parse("https://www.huya.com/770001").await;
    parse("https://live.douyin.com/770002").await;
    eventually(WAIT, "the Douyin event is delivered", || async {
        (subscribed.received().len() >= 2).then_some(())
    })
    .await;
    let received = subscribed.received();
    assert_eq!(
        received.len(),
        2,
        "the repeated Huya failure was not re-sent"
    );
    assert!(json_body(&received[1]).to_string().contains("douyin"));
    let logged = credential_unavailable_log(&app, &token).await;
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert!(
        unsubscribed.received().is_empty(),
        "a channel not subscribed to the event receives nothing"
    );

    app.shutdown().await;
}

/// Only the unconfigured default is testable: the VAPID keys are process
/// environment variables, which [`TestApp::start`] requires to be unset.
#[tokio::test]
async fn web_push_routes_report_service_unavailable_without_vapid_keys() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let subscribe = json!({
        "subscription": {
            "endpoint": "https://push.example.com/endpoint",
            "keys": { "p256dh": "key", "auth": "auth" },
        },
        "min_priority": 8,
    });
    let unsubscribe = json!({ "endpoint": "https://push.example.com/endpoint" });

    for (method, path, body) in [
        (Method::GET, "/api/notifications/web-push/public-key", None),
        (
            Method::GET,
            "/api/notifications/web-push/subscriptions",
            None,
        ),
        (
            Method::POST,
            "/api/notifications/web-push/subscribe",
            Some(&subscribe),
        ),
        (
            Method::POST,
            "/api/notifications/web-push/unsubscribe",
            Some(&unsubscribe),
        ),
    ] {
        let response = app.send(method, path, Some(&token), body).await;
        assert_eq!(
            response.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{path}: {}",
            response.text
        );
        assert_eq!(response.error_code(), Some("SERVICE_UNAVAILABLE"), "{path}");
    }

    app.shutdown().await;
}
