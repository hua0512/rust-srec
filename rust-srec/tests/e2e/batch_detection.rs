//! Batch detection: with a platform's `batch_detection` option on, routine
//! polls of offline streamers are answered from one batch status call for many
//! rooms instead of a status request per room.
//!
//! Every app here is seeded with [`Seed::live`]. The fake platform's batch
//! endpoint reports the same room states as its per-room status endpoint.

use serde_json::Value;

use crate::support::{FakePlatform, FlvLoop, Seed, TestApp, WAIT, eventually, sql_text};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offline_room_is_polled_through_batch_calls_only() {
    let fake = FakePlatform::start().await;
    let room = fake.room("quiet");
    let app = TestApp::start_with(with_batch_detection()).await;
    let token = app.admin_token().await;
    let id = app.watch(&token, "Quiet", &room, Value::Null).await;

    wait_for_batch_calls(&fake, 3).await;
    app.wait_for_more_checks(&token, &id, 2).await;
    assert_eq!(fake.requests_to(&room, "status").len(), 0);
    let checks = app.checks(&token, &id).await;
    assert!(
        checks.iter().all(|check| check["outcome"] == "offline"),
        "{checks:?}"
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
    app.watch(&token, "Quiet", &quiet, Value::Null).await;
    let busy_id = app.watch(&token, "Busy", &busy, Value::Null).await;

    busy.wait_for_media_connection(1).await;
    assert_eq!(app.streamer(&token, &busy_id).await["state"], "LIVE");
    eventually(WAIT, "individual checks of the offline room", || async {
        (fake.requests_to(&quiet, "status").len() >= 2).then_some(())
    })
    .await;
    assert!(fake.batch_calls() >= 1);

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
