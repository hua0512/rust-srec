//! Batch detection: with a platform's `batch_detection` option on, routine
//! polls of offline streamers are answered from one batch status call for many
//! rooms instead of a status request per room.
//!
//! Every app here is seeded with [`Seed::live`]. The fake platform's batch
//! endpoint reports the same room states as its per-room status endpoint.
//!
//! The check history marks each poll with how it was answered: `batch` or
//! `individual`.

use serde_json::Value;
use std::time::Duration;

use crate::download_progress::ws_message::Payload;
use crate::sockets::{next_payload, query_socket};
use crate::support::{FakePlatform, FlvLoop, Seed, Socket, TestApp, WAIT, eventually, sql_text};

fn with_batch_detection() -> Seed {
    Seed::live().platform(
        "platform_specific_config",
        sql_text(r#"{"batch_detection":true}"#),
    )
}

async fn wait_for_batch_calls(fake: &FakePlatform, count: usize) {
    eventually(WAIT, &format!("{count} batch status calls"), || async {
        (fake.batch_calls() >= count).then_some(())
    })
    .await;
}

/// Outcome and source of the streamer's next check-history frame on the
/// downloads socket.
async fn next_check(socket: &mut Socket, streamer_id: &str) -> (String, String) {
    next_payload(socket, "a check-history frame", |payload| match payload {
        Payload::StreamerCheckRecorded(check) if check.streamer_id == streamer_id => {
            Some((check.outcome, check.source))
        }
        _ => None,
    })
    .await
}

/// Every check-history row of the streamer is an offline poll from `source`.
async fn assert_offline_checks_from(app: &TestApp, token: &str, id: &str, source: &str) {
    let checks = app.checks(token, id).await;
    assert!(
        !checks.is_empty()
            && checks
                .iter()
                .all(|check| check["outcome"] == "offline" && check["source"] == source),
        "{checks:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offline_room_is_polled_through_batch_calls_only() {
    let fake = FakePlatform::start().await;
    let room = fake.room("quiet");
    let app = TestApp::start_with(with_batch_detection()).await;
    let token = app.admin_token().await;
    let mut socket = query_socket(&app, "/api/downloads/ws", &token).await;
    let id = app.watch(&token, "Quiet", &room, Value::Null).await;

    wait_for_batch_calls(&fake, 3).await;
    app.wait_for_more_checks(&token, &id, 2).await;
    assert_eq!(fake.requests_to(&room, "status").len(), 0);
    assert_offline_checks_from(&app, &token, &id, "batch").await;
    assert_eq!(
        next_check(&mut socket, &id).await,
        ("offline".to_owned(), "batch".to_owned())
    );

    app.shutdown().await;
}

/// The batch answer "live" leads to the room's full check, and once the
/// recording ends its offline confirmation asks the room itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_going_live_is_recorded_and_its_end_is_confirmed_per_room() {
    let fake = FakePlatform::start().await;
    let room = fake.room("rising");
    let app = TestApp::start_with(with_batch_detection()).await;
    let token = app.admin_token().await;
    let id = app.watch(&token, "Rising", &room, Value::Null).await;
    wait_for_batch_calls(&fake, 1).await;
    assert_eq!(fake.requests_to(&room, "status").len(), 0);

    room.set_live(room.live("Rising"));
    room.wait_for_media_connection(1).await;
    assert_eq!(app.streamer(&token, &id).await["state"], "LIVE");

    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    // Every status request from here on finds the room offline, and the
    // batch endpoint would report it offline too.
    let before_end = fake.requests_to(&room, "status").len();
    room.end();
    let session = app.ended_session(&token, &id).await;
    assert_eq!(
        session["events"]
            .as_array()
            .and_then(|events| events.last())
            .map(|event| &event["payload"]["cause"]["type"]),
        Some(&Value::from("streamer_offline")),
        "{session}"
    );
    assert!(
        fake.requests_to(&room, "status").len() > before_end,
        "the end was confirmed without asking the room"
    );

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_batch_starts_recording_before_the_rooms_next_poll() {
    let fake = FakePlatform::start().await;
    let room = fake.room("peer");
    let app =
        TestApp::start_with(with_batch_detection().global("streamer_check_delay_ms", 16_000)).await;
    let token = app.admin_token().await;
    let mut socket = query_socket(&app, "/api/downloads/ws", &token).await;
    next_payload(&mut socket, "the initial downloads snapshot", |payload| {
        matches!(payload, Payload::Snapshot(_)).then_some(())
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let id = app.watch(&token, "Peer", &room, Value::Null).await;
    assert_eq!(next_check(&mut socket, &id).await.1, "batch");
    room.set_live(room.live("Peer"));
    assert!(
        tokio::time::timeout(Duration::from_millis(4100), next_check(&mut socket, &id))
            .await
            .is_err()
    );

    app.watch(&token, "Trigger", &fake.room("trigger"), Value::Null)
        .await;
    tokio::time::timeout_at(deadline, room.wait_for_media_connection(1))
        .await
        .expect("peer discovery must beat the earliest routine poll");
    assert_eq!(fake.requests_to(&room, "status").len(), 1);
    assert_eq!(fake.batch_calls(), 2);
    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rooms_are_checked_individually_while_batch_calls_fail() {
    let fake = FakePlatform::start().await;
    fake.fail_batch_calls();
    let quiet = fake.room("quiet");
    let busy = fake.room("busy");
    busy.set_live(busy.live("Busy"));
    let app = TestApp::start_with(with_batch_detection()).await;
    let token = app.admin_token().await;
    let mut socket = query_socket(&app, "/api/downloads/ws", &token).await;
    let quiet_id = app.watch(&token, "Quiet", &quiet, Value::Null).await;
    let busy_id = app.watch(&token, "Busy", &busy, Value::Null).await;
    assert_eq!(
        next_check(&mut socket, &quiet_id).await,
        ("offline".to_owned(), "individual".to_owned())
    );

    busy.wait_for_media_connection(1).await;
    assert_eq!(app.streamer(&token, &busy_id).await["state"], "LIVE");
    eventually(WAIT, "individual checks of the offline room", || async {
        (fake.requests_to(&quiet, "status").len() >= 2).then_some(())
    })
    .await;
    assert!(fake.batch_calls() >= 1);
    assert_offline_checks_from(&app, &token, &quiet_id, "individual").await;

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_option_no_batch_call_is_made() {
    let fake = FakePlatform::start().await;
    let room = fake.room("plain");
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    app.watch(&token, "Plain", &room, Value::Null).await;

    eventually(WAIT, "individual checks of the room", || async {
        (fake.requests_to(&room, "status").len() >= 3).then_some(())
    })
    .await;
    assert_eq!(fake.batch_calls(), 0);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_identity_is_reused_after_restart_and_refreshed_after_url_change() {
    let fake = FakePlatform::start().await;
    let room = fake.room("identity-original");
    let app = TestApp::start_with(with_batch_detection()).await;
    let token = app.admin_token().await;
    let id = app.watch(&token, "Identity", &room, Value::Null).await;
    wait_for_batch_calls(&fake, 2).await;
    assert_eq!(fake.requests_to(&room, "status").len(), 1);

    let app = app.restart().await;
    let before = fake.batch_calls();
    wait_for_batch_calls(&fake, before + 2).await;
    assert_eq!(
        fake.requests_to(&room, "status").len(),
        1,
        "restart must hydrate the verified identity"
    );
    let checks = app.checks(&token, &id).await;
    assert_eq!(checks.last().unwrap()["source"], "batch");

    app.put(
        &format!("/api/streamers/{id}"),
        &token,
        &serde_json::json!({"name":"Renamed"}),
    )
    .await
    .ok();
    app.wait_for_more_checks(&token, &id, 2).await;
    assert_eq!(
        fake.requests_to(&room, "status").len(),
        1,
        "ordinary edits must preserve identity"
    );

    let replacement = fake.room("identity-replacement");
    app.put(
        &format!("/api/streamers/{id}"),
        &token,
        &serde_json::json!({"url":replacement.url()}),
    )
    .await
    .ok();
    eventually(WAIT, "the replacement identity to be resolved", || async {
        (!fake.requests_to(&replacement, "status").is_empty()).then_some(())
    })
    .await;
    app.wait_for_more_checks(&token, &id, 3).await;
    assert_eq!(
        app.checks(&token, &id).await.last().unwrap()["source"],
        "batch"
    );
    assert_eq!(fake.requests_to(&replacement, "status").len(), 1);
    app.shutdown().await;
}

/// Bigo has no batch status call: its batch detection is BIGO's live
/// notifications, whose listener reports how each Bigo streamer is monitored.
const BIGO_NOTIFICATIONS: &str = "/api/config/platforms/bigo/notifications";

/// The notification status once the listener has read the Bigo platform.
async fn bigo_notifications(app: &TestApp, token: &str, settled: impl Fn(&Value) -> bool) -> Value {
    eventually(WAIT, "the Bigo notification status", || async {
        let status = app.get_ok(BIGO_NOTIFICATIONS, token).await;
        settled(&status).then_some(status)
    })
    .await
}

/// Notifications are off until the platform turns batch detection on.
/// Without a notification account the listener cannot sign in, so every Bigo
/// streamer is polled, and the socket tells clients to refetch the status.
#[tokio::test]
async fn bigo_streamers_are_polled_without_a_notification_account() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let mut socket = query_socket(&app, "/api/downloads/ws", &token).await;
    let id = app
        .disabled_streamer(&token, "Bigo", "https://www.bigo.tv/100000001")
        .await;

    let status = bigo_notifications(&app, &token, |status| !status["platform_id"].is_null()).await;
    assert_eq!(status["platform_id"], "platform-bigo", "{status}");
    assert_eq!(status["state"], "disabled", "{status}");
    assert_eq!(status["reason"], "notifications_off", "{status}");

    let path = "/api/config/platforms/platform-bigo";
    let mut platform = app.get_ok(path, &token).await;
    platform["platform_specific_config"] = r#"{"batch_detection":true}"#.into();
    app.put(path, &token, &platform).await.ok();

    next_payload(
        &mut socket,
        "a Bigo notification status change",
        |payload| matches!(payload, Payload::BigoNotificationsChanged(_)).then_some(()),
    )
    .await;
    let status = bigo_notifications(&app, &token, |status| {
        status["reason"] == "no_account" && status["streamers"] != serde_json::json!([])
    })
    .await;
    assert_eq!(status["state"], "error", "{status}");
    assert_eq!(
        status["streamers"],
        serde_json::json!([{"streamer_id": id, "method": "polling", "reason": "no_account"}]),
        "{status}"
    );

    app.shutdown().await;
}
