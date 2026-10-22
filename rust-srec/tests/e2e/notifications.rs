//! Notification channels, subscriptions, the event catalog and log, and the
//! test button, driven over HTTP with a [`LocalHttpServer`] as every channel's
//! receiver.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    ADMIN_PASSWORD, ADMIN_USERNAME, FakePlatform, FlvLoop, LocalHttpServer, ReceivedRequest, Seed,
    SmtpFixture, TestApp, WAIT, assert_recent, eventually, id_of, millis, now_ms, sorted_keys,
    succeed,
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

/// Replace a channel's name and settings.
async fn update_channel(app: &TestApp, token: &str, id: &str, name: &str, settings: Value) {
    app.put(
        &format!("/api/notifications/channels/{id}"),
        token,
        &json!({ "name": name, "settings": settings }),
    )
    .await
    .expect_status(StatusCode::OK);
}

/// Replace a channel's subscriptions and return the response body.
async fn subscribe(app: &TestApp, token: &str, id: &str, events: &[&str]) -> Value {
    app.put(
        &format!("/api/notifications/channels/{id}/subscriptions"),
        token,
        &json!({ "events": events }),
    )
    .await
    .ok()
}

async fn test_send_ok(app: &TestApp, token: &str, id: &str) {
    app.test_send(token, id).await.expect_status(StatusCode::OK);
}

async fn loaded_channel_ids(app: &TestApp, token: &str) -> Vec<String> {
    app.get_ok("/api/notifications/instances", token)
        .await
        .as_array()
        .expect("instances array")
        .iter()
        .filter_map(|instance| instance["channel_id"].as_str().map(str::to_owned))
        .collect()
}

/// The stored settings, which the API returns as a JSON string.
fn stored_settings(channel: &Value) -> Value {
    let stored = channel["settings"]
        .as_str()
        .expect("settings come back as a JSON string");
    serde_json::from_str(stored).expect("settings JSON")
}

/// Email settings for a plaintext SMTP server without login, as English.
fn email_settings(host: &str, port: u16, username: &str) -> Value {
    json!({
        "smtp_host": host,
        "smtp_port": port,
        "username": username,
        "password": "",
        "from_address": "srec@example.com",
        "to_addresses": ["ops@example.com", "dev@example.com"],
        "use_tls": false,
        "min_priority": 5,
        "locale": "en",
    })
}

fn event_ids(body: &Value) -> Vec<&str> {
    body.as_array()
        .expect("event array")
        .iter()
        .map(|event| event["id"].as_str().expect("event id"))
        .collect()
}

#[tokio::test]
async fn webhook_test_send_delivers_the_documented_payload_headers_auth_kinds_and_locale() {
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
    test_send_ok(&app, &token, &id).await;

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
    let body = request.json();
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
    let basic = json!({
        "url": url,
        "method": "PUT",
        "auth": { "type": "Basic", "username": "user", "password": "pass" },
        "locale": "en",
    });
    update_channel(&app, &token, &id, "Ops webhook", basic).await;
    test_send_ok(&app, &token, &id).await;
    let received = receiver.received();
    assert_eq!(received.len(), 2);
    let request = &received[1];
    assert_eq!(request.method, "PUT");
    assert_eq!(request.header("authorization"), Some("Basic dXNlcjpwYXNz"));
    assert_eq!(request.header("x-a"), None);
    assert_eq!(request.json()["event_type"], "system_startup");

    // A custom auth header, with the method left to its POST default.
    let header = json!({
        "url": url,
        "auth": { "type": "Header", "name": "X-Token", "value": "header-secret" },
    });
    update_channel(&app, &token, &id, "Ops webhook", header).await;
    test_send_ok(&app, &token, &id).await;
    let received = receiver.received();
    assert_eq!(received.len(), 3);
    let request = &received[2];
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("x-token"), Some("header-secret"));
    assert_eq!(request.header("authorization"), None);

    // The channel locale decides the language of the test payload.
    let cases = [
        ("/zh", json!("zh-CN")),
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
        test_send_ok(&app, &token, &id).await;
    }

    let received = receiver.received();
    let payload = |path: &str| {
        let request = received
            .iter()
            .find(|request| request.path == path)
            .unwrap_or_else(|| panic!("{path} received the test send"));
        request.json()
    };
    assert_eq!(payload("/zh")["title"], ZH_TITLE);
    assert_eq!(payload("/zh")["description"], ZH_DESCRIPTION);
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

/// The Discord channel only takes high priorities (the email form's default
/// threshold, a rule every channel shares), yet receives the Normal-priority
/// test event: the test button is an explicit request.
#[tokio::test]
async fn discord_and_gotify_test_sends_reach_their_documented_endpoints() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "{}").await;
    let webhook_url = receiver.url("/discord/hook");

    let discord = create_channel(
        &app,
        &token,
        "Discord",
        "Discord",
        json!({ "webhook_url": webhook_url, "locale": "en", "min_priority": 8 }),
    )
    .await;
    test_send_ok(&app, &token, &discord).await;
    let received = receiver.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].method, "POST");
    assert_eq!(received[0].path, "/discord/hook");
    let body = received[0].json();
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
    test_send_ok(&app, &token, &gotify).await;
    let received = receiver.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[1].method, "POST");
    assert_eq!(received[1].path, "/gotify/message?token=gotify-token");
    assert_eq!(
        received[1].json(),
        json!({ "title": EN_TITLE, "message": EN_DESCRIPTION, "priority": 5 })
    );

    // A configured username and avatar override the webhook's own.
    let settings = json!({
        "webhook_url": webhook_url,
        "locale": "en",
        "min_priority": 8,
        "username": "Recorder",
        "avatar_url": "https://example.invalid/avatar.png",
    });
    update_channel(&app, &token, &discord, "Discord", settings).await;
    test_send_ok(&app, &token, &discord).await;
    let received = receiver.received();
    assert_eq!(received.len(), 3);
    let body = received[2].json();
    assert_eq!(sorted_keys(&body), ["avatar_url", "embeds", "username"]);
    assert_eq!(body["username"], "Recorder");
    assert_eq!(body["avatar_url"], "https://example.invalid/avatar.png");

    app.shutdown().await;
}

/// The bodies `channel-form.tsx` submits for each type, with its defaults:
/// integer `min_priority`, blank `locale`, explicit `enabled`. A webhook
/// stored with a string `min_priority` label loads too.
///
/// Older versions offered `config_updated`, so their backups still hold
/// subscriptions to it. Restoring one drops them under every spelling the
/// catalog accepted, while other names the catalog doesn't know are kept.
#[tokio::test]
async fn form_shaped_channels_load_send_and_survive_a_backup_that_drops_retired_subscriptions() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let receiver = LocalHttpServer::start(200, "{}").await;

    let channels = [
        (
            "Form Discord",
            "Discord",
            json!({
                "webhook_url": receiver.url("/discord"),
                "min_priority": 5,
                "locale": "",
                "enabled": true,
            }),
        ),
        (
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
        ),
        (
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
        ),
        (
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
        ),
        // `channel-form.tsx` turns auth `None` into null, and `removeEmpty` drops it.
        (
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
        ),
        (
            "Labelled webhook",
            "Webhook",
            json!({ "url": receiver.url("/labelled"), "min_priority": "low" }),
        ),
    ];
    let mut ids = Vec::new();
    for (name, channel_type, settings) in &channels {
        ids.push(create_channel(&app, &token, name, channel_type, settings.clone()).await);
    }

    let instances = app.get_ok("/api/notifications/instances", &token).await;
    let instances = instances.as_array().expect("instances array");
    for ((name, channel_type, _), id) in channels.iter().zip(&ids) {
        let instance = instances
            .iter()
            .find(|instance| instance["channel_id"] == id.as_str())
            .unwrap_or_else(|| panic!("{channel_type} channel is loaded: {instances:?}"));
        assert_eq!(instance["key"], id.as_str());
        assert_eq!(instance["display_name"], *name);
        assert_eq!(instance["channel_type"], *channel_type);
        assert_eq!(instance["source"], "database");
    }

    // Telegram and Email would leave the machine, so they don't send.
    for index in [0, 1, 4, 5] {
        test_send_ok(&app, &token, &ids[index]).await;
    }
    let paths: Vec<String> = receiver.received().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [
            "/discord",
            "/message?token=form-token",
            "/form",
            "/labelled"
        ]
    );

    let retired = &ids[4];
    let stored = [
        "ConfigUpdated",
        "config.updated",
        "config_updated",
        "future_event",
        "stream_online",
    ];
    let rows: Vec<String> = stored
        .iter()
        .map(|event| format!("('{retired}', '{event}')"))
        .collect();
    app.run_sql(&format!(
        "INSERT INTO notification_subscription (channel_id, event_name) VALUES {}",
        rows.join(", ")
    ))
    .await;

    // A backup holding them all imports again; the import reloads the
    // channels, subscriptions included.
    let bundle = app.get_ok("/api/config/backup/export", &token).await;
    let exported = bundle["notification_channels"]
        .as_array()
        .expect("exported channels")
        .iter()
        .find(|channel| channel["name"] == "Form webhook")
        .expect("the channel is exported");
    assert_eq!(exported["subscriptions"], json!(stored));
    let imported = app
        .post(
            "/api/config/backup/import",
            &token,
            &json!({ "config": bundle, "mode": "merge" }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(imported.body["stats"]["channels_updated"], 6);

    // A successful import ends every session.
    let token = app
        .login_tokens(ADMIN_USERNAME, ADMIN_PASSWORD)
        .await
        .access;
    let loaded = loaded_channel_ids(&app, &token).await;
    for id in &ids {
        assert!(loaded.contains(id), "{id} is loaded after the import");
    }
    let listed = app
        .get_ok(
            &format!("/api/notifications/channels/{retired}/subscriptions"),
            &token,
        )
        .await;
    assert_eq!(listed, json!(["future_event", "stream_online"]));

    app.shutdown().await;
}

/// `GET` returns the stored settings verbatim, secrets included, on purpose:
/// the edit form re-fills every field from it.
#[tokio::test]
async fn channel_and_subscription_crud_read_back_what_the_edit_form_needs_and_delete_cascades() {
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
    assert_eq!(stored_settings(&created.body), settings);

    let rejected = app
        .post(
            "/api/notifications/channels",
            &token,
            &json!({ "name": "Slack", "channel_type": "Slack", "settings": {} }),
        )
        .await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);

    let path = format!("/api/notifications/channels/{id}");
    assert_eq!(
        app.get_ok("/api/notifications/channels", &token).await,
        json!([created.body]),
        "the rejected type wrote nothing"
    );
    let fetched = app.get(&path, &token).await.expect_status(StatusCode::OK);
    assert_eq!(fetched.body, created.body);
    assert!(fetched.text.contains("crud-secret-token"));
    assert!(fetched.text.contains("header-secret"));

    let unknown = "/api/notifications/channels/00000000-0000-0000-0000-000000000000";
    app.put(
        unknown,
        &token,
        &json!({ "name": "Ghost", "settings": settings }),
    )
    .await
    .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let renamed_settings = json!({
        "url": "http://127.0.0.1:9/renamed",
        "auth": { "type": "Basic", "username": "ops", "password": "basic-secret" },
    });
    let updated = app
        .put(
            &path,
            &token,
            &json!({ "name": "Renamed webhook", "settings": renamed_settings }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(updated.body["id"], id.as_str());
    assert_eq!(updated.body["channel_type"], "Webhook");
    let fetched = app.get_ok(&path, &token).await;
    assert_eq!(fetched["name"], "Renamed webhook");
    assert_eq!(stored_settings(&fetched), renamed_settings);

    // Subscriptions echo the request and read back sorted, under canonical
    // names; a save replaces the whole set.
    let subscriptions = format!("{path}/subscriptions");
    let requested = ["stream_online", "StreamOffline", "download.error"];
    assert_eq!(
        subscribe(&app, &token, &id, &requested).await,
        json!(requested),
        "the response echoes the request"
    );
    assert_eq!(
        app.get_ok(&subscriptions, &token).await,
        json!(["download_error", "stream_offline", "stream_online"]),
        "legacy aliases are rewritten to canonical names on reload"
    );
    subscribe(&app, &token, &id, &["stream_online", "fatal_error"]).await;
    assert_eq!(
        app.get_ok(&subscriptions, &token).await,
        json!(["fatal_error", "stream_online"])
    );
    let unknown_subscriptions = format!("{unknown}/subscriptions");
    app.put(
        &unknown_subscriptions,
        &token,
        &json!({ "events": ["stream_online"] }),
    )
    .await
    .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(app.get_ok(&unknown_subscriptions, &token).await, json!([]));
    assert!(loaded_channel_ids(&app, &token).await.contains(&id));

    let deleted = app.delete(&path, &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    app.get(&path, &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(
        app.get_ok("/api/notifications/channels", &token).await,
        json!([])
    );
    assert!(!loaded_channel_ids(&app, &token).await.contains(&id));
    assert_eq!(app.get_ok(&subscriptions, &token).await, json!([]));
    let rows = app
        .query_i64(&format!(
            "SELECT COUNT(*) FROM notification_subscription WHERE channel_id = '{id}'"
        ))
        .await;
    assert_eq!(rows, 0, "deleting a channel removes its subscriptions");

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
    app.test_send(&token, &disabled)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
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
    let response = app.test_send(&token, &failing).await;
    response.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
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

/// A half-configured SMTP login and a blank host are refused before any
/// connection; a complete channel delivers through a local SMTP server.
#[tokio::test]
async fn email_test_send_checks_its_smtp_settings_before_connecting_and_delivers_locally() {
    let smtp = SmtpFixture::start().await;
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    // A plain listener: any SMTP connection attempt would sit in its backlog.
    let unused = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the SMTP listener");
    unused
        .set_nonblocking(true)
        .expect("make the SMTP listener non-blocking");
    let unused_port = unused.local_addr().expect("SMTP listener address").port();
    for (name, host, username) in [
        ("Half login", "127.0.0.1", "mailer"),
        ("Blank host", "   ", ""),
    ] {
        let id = create_channel(
            &app,
            &token,
            name,
            "Email",
            email_settings(host, unused_port, username),
        )
        .await;
        app.test_send(&token, &id)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    }
    match unused.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the SMTP server was contacted: {other:?}"),
    }

    let id = create_channel(
        &app,
        &token,
        "Mail",
        "Email",
        email_settings("127.0.0.1", smtp.port(), ""),
    )
    .await;
    test_send_ok(&app, &token, &id).await;
    let messages = smtp.messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let message = &messages[0];
    assert_eq!(message.from, "<srec@example.com>");
    assert_eq!(message.to, ["<ops@example.com>", "<dev@example.com>"]);
    let headers = message.data.split("\r\n\r\n").next().unwrap_or_default();
    for header in [
        "From: srec@example.com",
        "To: ops@example.com, dev@example.com",
        "Content-Type: multipart/alternative;",
    ] {
        assert!(headers.contains(header), "{header} missing in {headers}");
    }
    // The subject's emoji is an encoded word; the rest stays readable.
    assert!(
        headers
            .lines()
            .any(|line| line.starts_with("Subject: [rust-srec] ")
                && line.ends_with(" System started (vtest)")),
        "{headers}"
    );
    assert!(
        message
            .data
            .contains("System initialized successfully (vtest)"),
        "{}",
        message.data
    );

    app.shutdown().await;
}

/// `config_updated` is not offered: nothing emits it.
#[tokio::test]
async fn the_event_type_catalog_lists_canonical_types_with_integer_priorities() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let catalog = app.get_ok("/api/notifications/event-types", &token).await;
    let entries = catalog.as_array().expect("catalog array");
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
    assert!(!seen.contains("config_updated"), "{seen:?}");
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
async fn the_event_log_lists_newest_first_and_filters_and_pages() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let alpha = app
        .disabled_streamer(&token, "Alpha Caster", "https://www.huya.com/880001")
        .await;
    let beta = app
        .disabled_streamer(&token, "Beta Caster", "https://www.huya.com/880002")
        .await;

    // Nothing offline emits these, so seed the log the handler reads, one
    // second apart.
    let now = now_ms();
    let (a, b) = (Some(alpha.as_str()), Some(beta.as_str()));
    let rows = [
        (
            "stream_online",
            5,
            r#"{"StreamOnline":{"title":"Morning show"}}"#,
            a,
        ),
        (
            "download_error",
            8,
            r#"{"DownloadError":{"error_message":"Disk Quota exceeded"}}"#,
            b,
        ),
        (
            "fatal_error",
            10,
            r#"{"FatalError":{"error_type":"auth"}}"#,
            a,
        ),
        (
            "system_startup",
            5,
            r#"{"SystemStartup":{"version":"1.0.0"}}"#,
            None,
        ),
    ];
    let ids = [1, 2, 3, 4].map(|n| format!("00000000-0000-4000-8000-00000000000{n}"));
    let database = app.database().await;
    for (age, (id, (event_type, priority, payload, streamer_id))) in
        (1..=4).rev().zip(ids.iter().zip(rows))
    {
        sqlx::query(
            "INSERT INTO notification_event_log (id, event_type, priority, payload, streamer_id, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(event_type)
        .bind(priority)
        .bind(payload)
        .bind(streamer_id)
        .bind(now - age * 1000)
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
    let [e1, e2, e3, e4] = ids.each_ref().map(String::as_str);

    let all = app.get_ok("/api/notifications/events", &token).await;
    let ids = event_ids(&all);
    assert_eq!(ids.len(), 200, "the default page holds 200 events");
    assert_eq!(ids[..4], [e4, e3, e2, e1], "newest first");
    let entries = all.as_array().expect("events");
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
        entries[1]["payload"], rows[2].2,
        "the payload stays a string"
    );
    assert!(
        entries[0].get("streamer_id").is_none(),
        "no streamer, no key"
    );

    let largest = app
        .get_ok("/api/notifications/events?limit=5000", &token)
        .await;
    assert_eq!(
        largest.as_array().map(Vec::len),
        Some(1000),
        "the limit is capped at 1000"
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
    for (query, expected) in queries {
        let response = app
            .get_ok(&format!("/api/notifications/events?{query}"), &token)
            .await;
        assert_eq!(event_ids(&response), expected, "?{query}");
    }

    app.shutdown().await;
}

async fn credential_unavailable_log(app: &TestApp, token: &str) -> Vec<Value> {
    app.get_ok(
        "/api/notifications/events?event_type=credential_unavailable",
        token,
    )
    .await
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
    let critical_only = LocalHttpServer::start(200, "ok").await;
    for (name, receiver, event, min_priority) in [
        ("Subscribed", &subscribed, "credential_unavailable", 8),
        ("Unsubscribed", &unsubscribed, "stream_online", 5),
        (
            "Critical only",
            &critical_only,
            "credential_unavailable",
            10,
        ),
    ] {
        let id = create_channel(
            &app,
            &token,
            name,
            "Webhook",
            json!({ "url": receiver.url("/event"), "locale": "en", "min_priority": min_priority }),
        )
        .await;
        subscribe(&app, &token, &id, &[event]).await;
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
    let body = delivered[0].json();
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
    assert!(received[1].json().to_string().contains("douyin"));
    let logged = credential_unavailable_log(&app, &token).await;
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert!(
        unsubscribed.received().is_empty(),
        "a channel not subscribed to the event receives nothing"
    );
    assert!(
        critical_only.received().is_empty(),
        "a channel whose min_priority is above the event's receives nothing"
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

    for (method, route, body) in [
        (Method::GET, "public-key", None),
        (Method::GET, "subscriptions", None),
        (Method::POST, "subscribe", Some(&subscribe)),
        (Method::POST, "unsubscribe", Some(&unsubscribe)),
    ] {
        let path = format!("/api/notifications/web-push/{route}");
        let response = app.send(method, &path, Some(&token), body).await;
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

// Notifications a recording on the fake platform sends: they follow the
// session, its segments and its workflow, while the monitor itself notifies
// only fatal platform answers.

/// A webhook channel, at the lowest priority, subscribed to `events`.
async fn subscribed_webhook(
    app: &TestApp,
    token: &str,
    receiver: &LocalHttpServer,
    events: &[&str],
) {
    let channel = create_channel(
        app,
        token,
        "Recording alerts",
        "Webhook",
        json!({ "url": receiver.url("/hook"), "locale": "en", "min_priority": 2 }),
    )
    .await;
    subscribe(app, token, &channel, events).await;
}

/// The webhook bodies `receiver` got for `event_type`, in arrival order.
fn webhooks_of(receiver: &LocalHttpServer, event_type: &str) -> Vec<Value> {
    receiver
        .received()
        .iter()
        .map(ReceivedRequest::json)
        .filter(|body| body["event_type"] == event_type)
        .collect()
}

/// Wait until `receiver` has received `count` webhooks for `event_type`.
async fn wait_for_webhooks(receiver: &LocalHttpServer, event_type: &str, count: usize) {
    eventually(WAIT, &format!("{count} {event_type} webhooks"), || async {
        (webhooks_of(receiver, event_type).len() >= count).then_some(())
    })
    .await;
}

/// Wait for the streamer's newest session to end and return its detail.
async fn ended_session(app: &TestApp, token: &str, streamer_id: &str, count: usize) -> Value {
    let id = eventually(WAIT, "the session to end", || async {
        let sessions = app.sessions(token, streamer_id).await;
        (sessions.len() == count && sessions[0]["is_live"] == false).then(|| id_of(&sessions[0]))
    })
    .await;
    app.session(token, &id).await
}

/// Wait until the room's first recording part has been opened.
async fn wait_for_a_part(app: &TestApp) {
    eventually(WAIT, "a recording part to be opened", || async {
        (std::fs::read_dir(app.output_dir())
            .expect("read the output directory")
            .count()
            > 0)
        .then_some(())
    })
    .await;
}

/// The event types of the streamer's notification log, sorted.
async fn logged_event_types(app: &TestApp, token: &str, streamer_id: &str) -> Vec<String> {
    let log = app
        .get_ok(
            &format!("/api/notifications/events?streamer_id={streamer_id}"),
            token,
        )
        .await;
    let mut types: Vec<String> = log
        .as_array()
        .expect("event log")
        .iter()
        .map(|event| event["event_type"].as_str().expect("event type").to_owned())
        .collect();
    types.sort_unstable();
    types
}

fn timestamp(value: &Value) -> chrono::DateTime<chrono::FixedOffset> {
    let text = value.as_str().expect("an RFC 3339 timestamp");
    chrono::DateTime::parse_from_rfc3339(text).expect("an RFC 3339 timestamp")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recording_notifies_each_start_its_parts_and_workflow_and_one_offline_at_its_end() {
    let fake = FakePlatform::start().await;
    let room = fake.room("notified");
    room.set_live(room.live("Notified show").category("Games"));
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("after-session")))
            .await;
    let token = app.admin_token().await;
    // StreamOffline is a low-priority event, below the default minimum.
    subscribed_webhook(
        &app,
        &token,
        &receiver,
        &[
            "stream_online",
            "stream_offline",
            "segment_started",
            "segment_completed",
            "pipeline_started",
            "pipeline_completed",
        ],
    )
    .await;
    let id = id_of(
        &app.create_enabled_streamer(&token, json!({ "name": "Notified", "url": room.url() }))
            .await,
    );

    // A brief disconnect: the session waits in hysteresis and resumes.
    let mut flv = FlvLoop::tiny();
    room.wait_for_media_connection(1).await;
    room.push(flv.stream(1));
    wait_for_a_part(&app).await;
    room.reset();
    room.wait_for_media_connection(2).await;
    room.push(flv.stream(1));
    room.set_offline();
    room.end();
    let session = ended_session(&app, &token, &id, 1).await;
    // Channels deliver concurrently, so wait for each expected webhook. An
    // extra one would be dispatched next to the expected one of its kind.
    for (event_type, count) in [
        ("stream_online", 2),
        ("stream_offline", 1),
        ("segment_started", 2),
        ("segment_completed", 2),
        ("pipeline_started", 1),
        ("pipeline_completed", 1),
    ] {
        wait_for_webhooks(&receiver, event_type, count).await;
    }
    for request in receiver.received() {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/hook");
    }

    // Online for the start and again for the resumed start, offline once
    // when the session ended; each at the time the session recorded.
    let resumed = session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .find(|event| event["payload"]["from_hysteresis"] == true)
        .unwrap_or_else(|| panic!("no resumed start: {session}"));
    let online = webhooks_of(&receiver, "stream_online");
    assert_eq!(online.len(), 2, "{online:?}");
    assert_eq!(
        millis(&online[0]["timestamp"]),
        millis(&session["start_time"])
    );
    assert_eq!(
        millis(&online[1]["timestamp"]),
        millis(&resumed["occurred_at"])
    );
    let offline = webhooks_of(&receiver, "stream_offline");
    assert_eq!(offline.len(), 1, "{offline:?}");
    assert_eq!(
        millis(&offline[0]["timestamp"]),
        millis(&session["end_time"])
    );
    let stream_offline = &offline[0]["data"]["StreamOffline"];
    assert_eq!(stream_offline["streamer_name"], "Notified");
    assert_eq!(offline[0]["priority_label"], "low");
    // It reports how long the session lasted, the resumed part included.
    let duration = stream_offline["duration_secs"]
        .as_f64()
        .unwrap_or_else(|| panic!("no duration: {}", offline[0]));
    let lasted = millis(&session["end_time"]) - millis(&session["start_time"]);
    assert!(
        ((duration * 1000.0).round() as i64 - lasted).abs() <= 1,
        "{duration} s for a session of {lasted} ms"
    );
    for body in &online {
        assert_eq!(body["title"], "🔴 Notified is now live!", "{body}");
        assert_eq!(body["priority_label"], "normal");
        let stream_online = &body["data"]["StreamOnline"];
        assert_eq!(stream_online["streamer_name"], "Notified", "{body}");
        assert_eq!(stream_online["title"], "Notified show", "{body}");
        assert_eq!(stream_online["category"], "Games", "{body}");
    }
    for body in online.iter().chain(&offline) {
        assert_eq!(body["streamer_id"], id.as_str(), "{body}");
    }

    // Each part's webhooks carry the times its segment row records.
    let session_id = id_of(&session);
    let mut segments = app
        .get_items(&format!("/api/sessions/{session_id}/segments"), &token)
        .await;
    segments.sort_by_key(|segment| segment["segment_index"].as_i64());
    assert_eq!(segments.len(), 2, "{segments:?}");
    for (event_type, variant, time) in [
        ("segment_started", "SegmentStarted", "created_at"),
        ("segment_completed", "SegmentCompleted", "completed_at"),
    ] {
        let mut bodies = webhooks_of(&receiver, event_type);
        bodies.sort_by_key(|body| body["data"][variant]["segment_index"].as_i64());
        assert_eq!(bodies.len(), 2, "{bodies:?}");
        for (body, segment) in bodies.iter().zip(&segments) {
            assert_eq!(body["streamer_id"], id.as_str(), "{body}");
            assert_eq!(body["data"][variant]["segment_path"], segment["file_path"]);
            assert_eq!(
                millis(&body["timestamp"]),
                millis(&segment[time]),
                "{event_type}: {body} {segment}"
            );
        }
    }

    // The session-complete step's job starts and completes.
    let jobs = app
        .get_items(
            &format!("/api/pipeline/jobs?session_id={session_id}"),
            &token,
        )
        .await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let started = webhooks_of(&receiver, "pipeline_started");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["streamer_id"], id.as_str());
    assert_eq!(
        started[0]["data"]["PipelineStarted"]["job_id"],
        jobs[0]["id"]
    );
    assert_eq!(started[0]["data"]["PipelineStarted"]["job_type"], "execute");
    let completed = webhooks_of(&receiver, "pipeline_completed");
    assert_eq!(completed.len(), 1, "{completed:?}");
    assert_eq!(
        completed[0]["data"]["PipelineCompleted"]["job_id"],
        jobs[0]["id"]
    );
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_a_recording_streamer_notifies_no_offline() {
    let fake = FakePlatform::start().await;
    let room = fake.room("disabled");
    room.set_live(room.live("Interrupted"));
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    subscribed_webhook(
        &app,
        &token,
        &receiver,
        &["stream_online", "stream_offline"],
    )
    .await;
    let id = id_of(
        &app.create_enabled_streamer(&token, json!({ "name": "Disabled", "url": room.url() }))
            .await,
    );
    let path = format!("/api/streamers/{id}");

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    wait_for_a_part(&app).await;
    app.put(&path, &token, &json!({ "enabled": false }))
        .await
        .expect_status(StatusCode::OK);
    let stopped = ended_session(&app, &token, &id, 1).await;
    let ended = stopped["events"]
        .as_array()
        .expect("session events")
        .iter()
        .find(|event| event["kind"] == "session_ended")
        .unwrap_or_else(|| panic!("no session_ended event: {stopped}"));
    assert_eq!(
        ended["payload"]["cause"],
        json!({ "type": "user_disabled" }),
        "{stopped}"
    );

    // Enabled again, the streamer records the still-live room in a second
    // session, which ends because the room goes offline.
    app.put(&path, &token, &json!({ "enabled": true }))
        .await
        .expect_status(StatusCode::OK);
    room.wait_for_media_connection(2).await;
    room.broadcast_ends(1);
    let finished = ended_session(&app, &token, &id, 2).await;
    wait_for_webhooks(&receiver, "stream_online", 2).await;
    wait_for_webhooks(&receiver, "stream_offline", 1).await;

    // The disabled session ended seconds before the second one went offline
    // (a restart, a recording and an offline confirmation lie between), so a
    // stream_offline for it would have arrived by now.
    assert_eq!(receiver.received().len(), 3);
    let offline = webhooks_of(&receiver, "stream_offline");
    assert_eq!(offline.len(), 1, "{offline:?}");
    assert_eq!(
        millis(&offline[0]["timestamp"]),
        millis(&finished["end_time"])
    );

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_monitor_notifies_only_a_fatal_platform_answer_at_the_time_it_was_seen() {
    let fake = FakePlatform::start().await;
    let room = fake.room("monitored");
    room.set_live(room.live("Rerun tonight"));
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    subscribed_webhook(&app, &token, &receiver, &["fatal_error"]).await;

    // The streamer is held back by a filter first (a state change), then
    // goes live, records and goes offline, and finally the platform says it
    // does not exist.
    let id = app.disabled_streamer(&token, "Monitored", room.url()).await;
    app.post(
        &format!("/api/streamers/{id}/filters"),
        &token,
        &json!({
            "streamer_id": id,
            "filter_type": "KEYWORD",
            "config": { "include": [], "exclude": ["rerun"] },
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    app.put(
        &format!("/api/streamers/{id}"),
        &token,
        &json!({ "enabled": true }),
    )
    .await
    .expect_status(StatusCode::OK);
    eventually(WAIT, "the streamer to be held back", || async {
        let streamer = app.streamer(&token, &id).await;
        (streamer["state"] == "OUT_OF_SCHEDULE").then_some(())
    })
    .await;
    room.set_live(room.live("Live tonight"));
    room.wait_for_media_connection(1).await;
    room.broadcast_ends(1);
    ended_session(&app, &token, &id, 1).await;
    room.set_error("not_found", "gone");
    wait_for_webhooks(&receiver, "fatal_error", 1).await;

    // The notification carries the monitor's recorded event: its type, its
    // message and the time the answer was seen.
    let database = app.database().await;
    let recorded: String = sqlx::query_scalar(
        "SELECT payload FROM monitor_event_outbox WHERE streamer_id = ? AND event_type = 'FatalError'",
    )
    .bind(&id)
    .fetch_one(&database)
    .await
    .expect("the recorded fatal event");
    database.close().await;
    let recorded = &serde_json::from_str::<Value>(&recorded).expect("JSON payload")["FatalError"];
    let fatal = &webhooks_of(&receiver, "fatal_error")[0];
    assert_eq!(fatal["streamer_id"], id.as_str());
    assert_eq!(
        fatal["data"]["FatalError"]["error_type"], "NotFound",
        "{fatal}"
    );
    assert_eq!(fatal["data"]["FatalError"]["message"], recorded["message"]);
    assert_eq!(
        timestamp(&fatal["timestamp"]),
        timestamp(&recorded["timestamp"])
    );

    // Everything else the streamer was notified of came from its session.
    let expected = [
        "download_completed",
        "download_started",
        "fatal_error",
        "segment_completed",
        "segment_started",
        "stream_offline",
        "stream_online",
    ];
    let types = eventually(WAIT, "the session's notifications in the log", || async {
        let types = logged_event_types(&app, &token, &id).await;
        expected
            .iter()
            .all(|expected| types.iter().any(|logged| logged == expected))
            .then_some(types)
    })
    .await;
    assert_eq!(types, expected);

    app.shutdown().await;
}

/// The decoded body of the `content_type` part of a multipart email.
fn email_part(data: &str, content_type: &str) -> String {
    use base64::Engine as _;

    let start = data
        .find(&format!("Content-Type: {content_type}"))
        .unwrap_or_else(|| panic!("no {content_type} part in {data}"));
    let (headers, rest) = data[start..].split_once("\r\n\r\n").expect("part headers");
    let body = rest.split("\r\n--").next().expect("part body");
    let bytes = if headers.contains("Content-Transfer-Encoding: base64") {
        base64::engine::general_purpose::STANDARD
            .decode(body.replace("\r\n", ""))
            .expect("base64 body")
    } else if headers.contains("Content-Transfer-Encoding: quoted-printable") {
        let soft = body.replace("=\r\n", "");
        let mut bytes = Vec::new();
        let mut input = soft.bytes();
        while let Some(byte) = input.next() {
            if byte == b'=' {
                let hex = [input.next().expect("hex"), input.next().expect("hex")];
                let hex = std::str::from_utf8(&hex).expect("hex digits");
                bytes.push(u8::from_str_radix(hex, 16).expect("hex escape"));
            } else {
                bytes.push(byte);
            }
        }
        bytes
    } else {
        body.as_bytes().to_vec()
    };
    String::from_utf8(bytes).expect("UTF-8 body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_online_email_escapes_the_live_title_in_its_html_part() {
    let smtp = SmtpFixture::start().await;
    let fake = FakePlatform::start().await;
    let room = fake.room("mailed");
    room.set_live(room.live(r#"<b>Tom & "Jerry"</b>"#).category("Talk"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let mut settings = email_settings("127.0.0.1", smtp.port(), "");
    settings["locale"] = json!("zh-CN");
    // A setting older versions had, which this one ignores.
    settings["batch_window_secs"] = json!(60);
    let channel = create_channel(&app, &token, "Mail", "Email", settings).await;
    subscribe(&app, &token, &channel, &["stream_online"]).await;
    app.create_enabled_streamer(&token, json!({ "name": "Mailed", "url": room.url() }))
        .await;

    let message = eventually(WAIT, "the stream_online email", || async {
        smtp.messages().pop()
    })
    .await;
    // The zh-CN description wraps the category in full-width parentheses.
    let plain = email_part(&message.data, "text/plain");
    assert!(plain.contains("Mailed 开播了"), "{plain}");
    assert!(plain.contains(r#"<b>Tom & "Jerry"</b>（Talk）"#), "{plain}");
    let html = email_part(&message.data, "text/html");
    assert!(
        html.contains("&lt;b&gt;Tom &amp; &quot;Jerry&quot;&lt;/b&gt;（Talk）"),
        "{html}"
    );
    assert!(!html.contains("<b>Tom"), "{html}");

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_fatal_platform_answer_is_notified_with_its_own_error_type() {
    let fake = FakePlatform::start().await;
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    subscribed_webhook(&app, &token, &receiver, &["fatal_error"]).await;
    let cases = [
        ("not_found", "NotFound"),
        ("banned", "Banned"),
        ("age_restricted", "AgeRestricted"),
        ("region_locked", "RegionLocked"),
        ("private", "Private"),
    ];
    let mut watched = Vec::new();
    for (kind, error_type) in cases {
        let room = fake.room(kind);
        room.set_error(kind, "the platform says no");
        let id = id_of(
            &app.create_enabled_streamer(&token, json!({ "name": kind, "url": room.url() }))
                .await,
        );
        watched.push((id, error_type));
    }

    wait_for_webhooks(&receiver, "fatal_error", cases.len()).await;
    let fatal = webhooks_of(&receiver, "fatal_error");
    assert_eq!(fatal.len(), cases.len(), "{fatal:?}");
    for (id, error_type) in &watched {
        let body = fatal
            .iter()
            .find(|body| body["streamer_id"] == id.as_str())
            .unwrap_or_else(|| panic!("no fatal_error for {error_type}: {fatal:?}"));
        assert_eq!(
            body["data"]["FatalError"]["error_type"], *error_type,
            "{body}"
        );
    }

    app.shutdown().await;
}
