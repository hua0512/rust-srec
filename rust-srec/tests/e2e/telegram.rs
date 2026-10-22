//! The Telegram channel against a local fake Bot API, which each test points
//! its bot token at with the backend's API-base override.

use axum::response::IntoResponse;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    FakePlatform, LocalHttpServer, ReceivedRequest, Seed, TestApp, WAIT, eventually, id_of,
};

/// A fake Bot API answering every `sendMessage` like Telegram does.
async fn bot_api(status: u16, reply: Value) -> LocalHttpServer {
    let status = axum::http::StatusCode::from_u16(status).expect("valid status");
    LocalHttpServer::start_with(move |_, _| (status, axum::Json(reply.clone())).into_response())
        .await
}

fn sent_message() -> Value {
    json!({ "ok": true, "result": { "message_id": 1, "chat": { "id": -1001234567890_i64 } } })
}

/// A Telegram channel for `bot_token` whose messages go to `api`.
async fn telegram_channel(
    app: &TestApp,
    token: &str,
    api: &LocalHttpServer,
    bot_token: &str,
    chat_id: &str,
) -> String {
    rust_srec::notification::channels::override_telegram_api_base(bot_token, &api.url(""));
    let response = app
        .post(
            "/api/notifications/channels",
            token,
            &json!({
                "name": "Telegram alerts",
                "channel_type": "Telegram",
                "settings": {
                    "bot_token": bot_token,
                    "chat_id": chat_id,
                    "parse_mode": "HTML",
                    "min_priority": 5,
                    "locale": "en",
                    "enabled": true,
                },
            }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    id_of(&response.body)
}

async fn test_send(app: &TestApp, token: &str, id: &str) -> crate::support::Response {
    app.send(
        Method::POST,
        &format!("/api/notifications/channels/{id}/test"),
        Some(token),
        None,
    )
    .await
}

fn json_body(request: &ReceivedRequest) -> Value {
    serde_json::from_slice(&request.body).expect("JSON request body")
}

/// The UTF-16 span an entity marks in a message's text.
fn entity_text(message: &Value, entity: &Value) -> String {
    let units: Vec<u16> = message["text"]
        .as_str()
        .expect("message text")
        .encode_utf16()
        .collect();
    let offset = entity["offset"].as_u64().expect("entity offset") as usize;
    let length = entity["length"].as_u64().expect("entity length") as usize;
    String::from_utf16(&units[offset..offset + length]).expect("entity spans whole characters")
}

#[tokio::test]
async fn telegram_test_send_posts_the_message_for_the_chat_to_the_bot_api() {
    let api = bot_api(200, sent_message()).await;
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let bot_token = "4242:e2e-test-send-token";
    let channel = telegram_channel(&app, &token, &api, bot_token, "-1001234567890").await;

    test_send(&app, &token, &channel)
        .await
        .expect_status(StatusCode::OK);

    let received = api.received();
    assert_eq!(received.len(), 1, "{received:?}");
    let request = &received[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, format!("/bot{bot_token}/sendMessage"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    let message = json_body(request);
    assert_eq!(message["chat_id"], "-1001234567890");
    assert_eq!(
        message["text"],
        "🔔 🚀 System started (vtest)\n\nSystem initialized successfully (vtest)\n\n\
         Priority: normal | Type: system_startup",
        "{message}"
    );
    // Formatting rides in entities, never in parse_mode, so the text stays literal.
    assert!(message.get("parse_mode").is_none(), "{message}");
    let entities = message["entities"].as_array().expect("entities");
    assert_eq!(entities.len(), 2, "{message}");
    assert_eq!(entities[0]["type"], "bold");
    assert_eq!(
        entity_text(&message, &entities[0]),
        "🚀 System started (vtest)"
    );
    assert_eq!(entities[1]["type"], "italic");
    assert_eq!(
        entity_text(&message, &entities[1]),
        "Priority: normal | Type: system_startup"
    );

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_online_event_reaches_the_configured_telegram_chat() {
    let fake = FakePlatform::start().await;
    let room = fake.room("telegram");
    room.set_live(room.live("Late night <b>speedruns</b>").category("Games"));
    let api = bot_api(200, sent_message()).await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let bot_token = "4343:e2e-stream-online-token";
    let channel = telegram_channel(&app, &token, &api, bot_token, "@srec_alerts").await;
    app.put(
        &format!("/api/notifications/channels/{channel}/subscriptions"),
        &token,
        &json!({ "events": ["stream_online"] }),
    )
    .await
    .expect_status(StatusCode::OK);

    app.create_enabled_streamer(&token, json!({ "name": "Runner", "url": room.url() }))
        .await;

    let received = eventually(WAIT, "the stream_online message", || async {
        let received = api.received();
        (!received.is_empty()).then_some(received)
    })
    .await;
    assert_eq!(received[0].path, format!("/bot{bot_token}/sendMessage"));
    let message = json_body(&received[0]);
    assert_eq!(message["chat_id"], "@srec_alerts");
    let text = message["text"].as_str().expect("message text");
    assert!(
        text.starts_with("🔔 🔴 Runner is now live!\n\n"),
        "{message}"
    );
    assert!(
        text.contains("Late night <b>speedruns</b>"),
        "the live title is sent literally: {message}"
    );
    assert!(
        text.ends_with("Priority: normal | Type: stream_online"),
        "{message}"
    );
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}

#[tokio::test]
async fn a_bot_api_error_fails_the_test_send_without_echoing_the_bot_token_or_reply() {
    let api = bot_api(
        400,
        json!({ "ok": false, "error_code": 400, "description": "Bad Request: chat not found" }),
    )
    .await;
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let bot_token = "4444:e2e-private-bot-token";
    let channel = telegram_channel(&app, &token, &api, bot_token, "-1009999").await;

    let response = test_send(&app, &token, &channel)
        .await
        .expect_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.error_code(), Some("INTERNAL_ERROR"));
    for leaked in ["e2e-private-bot-token", "chat not found", "-1009999"] {
        assert!(
            !response.text.contains(leaked),
            "the error echoes {leaked}: {}",
            response.text
        );
    }
    assert_eq!(api.received().len(), 1, "a 400 is not retried");

    app.shutdown().await;
}
