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
