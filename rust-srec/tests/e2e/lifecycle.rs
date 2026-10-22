//! Streamer and session lifecycle on the fake platform: sessions a restart
//! left open, offline checks, disconnect classification, disabling, streamer
//! updates and deletion, and the download queue.
//!
//! Apps that monitor streamers are seeded with [`Seed::live`]; rows the
//! services load at startup (a streamer's state and counters, a session a
//! restart left open) are added with [`Seed::rows`].

use std::path::Path;

use futures::StreamExt;
use prost::Message as _;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::support::{
    FakePlatform, FakeRoom, FlvLoop, LocalHttpServer, ReceivedRequest, Seed, TestApp, WAIT,
    eventually, id_of, millis, now_ms, seeded_streamer, succeed, with_query,
};

/// Apply `change` to the streamer and return the updated streamer.
async fn update(app: &TestApp, token: &str, id: &str, change: &Value) -> Value {
    app.put(&format!("/api/streamers/{id}"), token, change)
        .await
        .ok()
}

/// Wait for the session to end and return its detail.
async fn ended(app: &TestApp, token: &str, id: &str) -> Value {
    eventually(WAIT, &format!("session {id} to end"), || async {
        let session = app.session(token, id).await;
        (session["is_live"] == false).then_some(session)
    })
    .await
}

/// The payloads of the session's `session_ended` events.
fn end_payloads(session: &Value) -> Vec<&Value> {
    session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .filter(|event| event["kind"] == "session_ended")
        .map(|event| &event["payload"])
        .collect()
}

/// A `session_ended` payload whose cause has only a type.
fn end(cause: &str, via_hysteresis: bool) -> Value {
    json!({ "kind": "session_ended", "cause": { "type": cause }, "via_hysteresis": via_hysteresis })
}

/// The titles a session recorded, oldest first.
fn titles(session: &Value) -> Vec<&str> {
    session["titles"]
        .as_array()
        .expect("session titles")
        .iter()
        .map(|entry| entry["title"].as_str().expect("title"))
        .collect()
}

/// A session row still recording, as a restart leaves it, titled `title`.
fn open_session(id: &str, streamer_id: &str, title: &str) -> String {
    let started = now_ms() - 600_000;
    format!(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             titles, total_size_bytes, session_complete_dispatched) \
         VALUES ('{id}', '{streamer_id}', '{streamer_id}', {started}, NULL, \
             '[{{\"ts\":{started},\"title\":\"{title}\"}}]', 0, 0);\n"
    )
}

/// A part the session recorded before the restart.
fn recorded_part(session_id: &str) -> String {
    let at = now_ms() - 300_000;
    format!(
        "INSERT INTO session_segments (id, session_id, segment_index, file_path, duration_secs, \
             size_bytes, created_at, completed_at, persisted_at) \
         VALUES ('{session_id}-part', '{session_id}', 0, '/recordings/{session_id}.flv', 10.0, \
             1000, {at}, {at}, {at});\n"
    )
}

/// A webhook channel to `receiver` subscribed to `events`, inserted before
/// boot so it sees the first checks.
fn webhook_channel(receiver: &LocalHttpServer, events: &[&str]) -> String {
    let settings = json!({
        "enabled": true,
        "url": receiver.url("/hook"),
        "min_priority": 2,
        "locale": "en",
    });
    let mut sql = format!(
        "INSERT INTO notification_channel (id, name, channel_type, settings) \
         VALUES ('hook', 'Hook', 'Webhook', '{settings}');\n"
    );
    for event in events {
        sql.push_str(&format!(
            "INSERT INTO notification_subscription (channel_id, event_name) VALUES ('hook', '{event}');\n"
        ));
    }
    sql
}

fn webhook_events(received: &[ReceivedRequest], streamer_id: &str) -> Vec<String> {
    received
        .iter()
        .map(ReceivedRequest::json)
        .filter(|body| body["streamer_id"] == streamer_id)
        .map(|body| body["event_type"].as_str().expect("event type").to_owned())
        .collect()
}

/// After a restart a session a recording left open is in the database only.
/// The first check decides it: offline ends it once, as an offline end that
/// runs the session-complete workflow on the part it recorded and sends the
/// offline notification; live reuses it, adding the room's title when it
/// changed. An offline room with no session gets neither a session nor a
/// notification.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_check_after_a_restart_ends_or_reuses_the_session_it_left_open() {
    let fake = FakePlatform::start().await;
    let gone = fake.room("gone");
    let back = fake.room("back");
    back.set_live(back.live("After the restart"));
    let same = fake.room("same");
    same.set_live(same.live("Before the restart"));
    let quiet = fake.room("quiet");
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app = TestApp::start_with(
        Seed::live()
            .session_complete_pipeline(&succeed("after-restart"))
            .rows(&format!(
                "{}{}{}{}{}{}{}{}{}",
                seeded_streamer("gone", &gone, "LIVE", &[]),
                open_session("gone-session", "gone", "Before the restart"),
                recorded_part("gone-session"),
                seeded_streamer("back", &back, "LIVE", &[]),
                open_session("back-session", "back", "Before the restart"),
                seeded_streamer("same", &same, "LIVE", &[]),
                open_session("same-session", "same", "Before the restart"),
                seeded_streamer("quiet", &quiet, "NOT_LIVE", &[]),
                webhook_channel(&receiver, &["stream_online", "stream_offline"]),
            )),
    )
    .await;
    let token = app.admin_token().await;

    let gone_session = ended(&app, &token, "gone-session").await;
    assert_eq!(
        end_payloads(&gone_session),
        [&end("streamer_offline", false)]
    );
    let dag = eventually(WAIT, "the session-complete workflow", || async {
        let dags = app.dags(&token, "gone-session").await;
        dags.first()
            .filter(|dag| dag["status"] == "COMPLETED")
            .cloned()
    })
    .await;
    assert_eq!(dag["name"], "after-restart");

    // A live room's check reuses the open session and records the title
    // when it changed.
    for (id, room, expected_titles) in [
        (
            "back",
            &back,
            &["Before the restart", "After the restart"][..],
        ),
        ("same", &same, &["Before the restart"][..]),
    ] {
        room.wait_for_media_connection(1).await;
        let reused = app.sessions(&token, id).await;
        assert_eq!(reused.len(), 1, "{reused:?}");
        let reused = app.session(&token, &format!("{id}-session")).await;
        assert_eq!(reused["is_live"], true, "{reused}");
        assert_eq!(end_payloads(&reused), Vec::<&Value>::new());
        assert_eq!(titles(&reused), expected_titles, "{id}");
        assert_eq!(app.streamer(&token, id).await["state"], "LIVE");
    }

    // Later offline checks change nothing for the ended session.
    app.wait_for_more_checks(&token, "gone", 2).await;
    app.wait_for_more_checks(&token, "quiet", 2).await;
    let gone_session = app.session(&token, "gone-session").await;
    assert_eq!(end_payloads(&gone_session).len(), 1, "{gone_session}");
    assert_eq!(app.dags(&token, "gone-session").await.len(), 1);
    assert_eq!(app.sessions(&token, "quiet").await, Vec::<Value>::new());
    for id in ["gone", "quiet"] {
        let history = app.checks(&token, id).await;
        assert!(
            history.iter().all(|check| check["outcome"] == "offline"),
            "{id}: {history:?}"
        );
    }

    // The next live opens a new session.
    gone.set_live(gone.live("Next broadcast"));
    gone.wait_for_media_connection(1).await;
    let gone_sessions = app.sessions(&token, "gone").await;
    assert_eq!(gone_sessions.len(), 2, "{gone_sessions:?}");
    assert_ne!(gone_sessions[0]["id"], "gone-session");
    assert_eq!(gone_sessions[0]["is_live"], true);

    // Notifications are sent concurrently, but the earlier ones were raised
    // seconds before the new session's online one, so once that has arrived
    // the others have too.
    let received = eventually(WAIT, "the online notification", || async {
        let received = receiver.received();
        webhook_events(&received, "gone")
            .contains(&"stream_online".to_owned())
            .then_some(received)
    })
    .await;
    assert_eq!(webhook_events(&received, "quiet"), Vec::<String>::new());
    let mut gone_events = webhook_events(&received, "gone");
    gone_events.sort();
    assert_eq!(gone_events, ["stream_offline", "stream_online"]);

    app.shutdown().await;
}

/// An offline check settles what earlier failures left: a streamer that
/// failed once, and one whose backoff has run out, return to NOT_LIVE with
/// their errors cleared. A backed-off streamer's session a restart left open
/// ends as an offline end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offline_check_clears_earlier_failures_and_ends_a_session_a_backoff_left_open() {
    let fake = FakePlatform::start().await;
    let failed = fake.room("failed");
    let backed_off = fake.room("backed-off");
    let recording = fake.room("recording");
    let expired = (now_ms() - 1).to_string();
    let backoff = |count: &'static str| {
        [
            ("consecutive_error_count", count),
            ("last_error", "'Extraction failed (http)'"),
            ("disabled_until", expired.as_str()),
        ]
    };
    let app = TestApp::start_with(Seed::live().rows(&format!(
        "{}{}{}{}",
        seeded_streamer(
            "failed",
            &failed,
            "NOT_LIVE",
            &[
                ("consecutive_error_count", "1"),
                ("last_error", "'Extraction failed (http)'"),
            ],
        ),
        seeded_streamer(
            "backed-off",
            &backed_off,
            "TEMPORAL_DISABLED",
            &backoff("2")
        ),
        seeded_streamer("recording", &recording, "TEMPORAL_DISABLED", &backoff("3")),
        open_session("stale-session", "recording", "Before the backoff"),
    )))
    .await;
    let token = app.admin_token().await;

    for id in ["failed", "backed-off", "recording"] {
        let cleared = eventually(WAIT, &format!("{id}'s errors to clear"), || async {
            let streamer = app.streamer(&token, id).await;
            (streamer["consecutive_error_count"] == 0).then_some(streamer)
        })
        .await;
        assert_eq!(cleared["state"], "NOT_LIVE", "{id}: {cleared}");
        assert!(cleared["last_error"].is_null(), "{id}: {cleared}");
        assert!(cleared["disabled_until"].is_null(), "{id}: {cleared}");
        assert_eq!(app.checks(&token, id).await[0]["outcome"], "offline");
    }
    let stale = ended(&app, &token, "stale-session").await;
    assert_eq!(end_payloads(&stale), [&end("streamer_offline", false)]);
    for id in ["failed", "backed-off"] {
        let opened = app.sessions(&token, id).await;
        assert!(opened.is_empty(), "{id}: {opened:?}");
    }

    app.shutdown().await;
}

/// The streamer's only session id, once it has one.
async fn only_session(app: &TestApp, token: &str, streamer_id: &str) -> String {
    eventually(WAIT, "the session", || async {
        let sessions = app.sessions(token, streamer_id).await;
        assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
        sessions
            .first()
            .map(|session| session["id"].as_str().expect("session id").to_owned())
    })
    .await
}

/// The end of a session two dropped connections in a row ended.
fn consecutive_failures_end() -> Value {
    json!({
        "kind": "session_ended",
        "cause": {
            "type": "definitive_offline",
            "signal": { "type": "consecutive_failures", "count": 2 },
        },
        "via_hysteresis": false,
    })
}

/// Two dropped connections with no recorded part between them are taken as
/// the broadcast ending: the session ends at once, without waiting for a
/// status check. A title first seen when the first drop resumes joins the
/// session's title list, so the session is named by it from then on.
///
/// The failures count within `offline_check_count` offline-check intervals,
/// widened here to 5 s so a loaded machine still drops the second connection
/// inside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_dropped_connections_without_a_recorded_part_end_the_session() {
    let fake = FakePlatform::start().await;
    let room = fake.room("flaky");
    room.set_live(room.live("First title"));
    let app = TestApp::start_with(Seed::live().global("offline_check_delay_ms", 5_000)).await;
    let token = app.admin_token().await;
    let id = app.watch(&token, "Flaky", &room, Value::Null).await;

    room.wait_for_media_connection(1).await;
    let session_id = only_session(&app, &token, &id).await;
    room.set_live(room.live("Second title"));
    room.reset();
    // The resume records the new title before it reconnects.
    room.wait_for_media_connection(2).await;
    room.reset();

    let session = ended(&app, &token, &session_id).await;
    assert_eq!(end_payloads(&session), [&consecutive_failures_end()]);
    assert_eq!(session["title"], "Second title", "{session}");
    assert_eq!(titles(&session), ["First title", "Second title"]);
    assert_eq!(app.sessions(&token, &id).await.len(), 1);

    app.shutdown().await;
}

/// A part recorded between two dropped connections restarts the count: the
/// session resumes after each and the streamer reads LIVE again. The drop
/// after the part is the first of a new run, so one more drop without a part
/// ends the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_part_recorded_between_dropped_connections_keeps_the_session() {
    let fake = FakePlatform::start().await;
    let room = fake.room("steady");
    room.set_live(room.live("First title"));
    let app = TestApp::start_with(Seed::live().global("offline_check_delay_ms", 5_000)).await;
    let token = app.admin_token().await;
    let config = json!({ "max_part_size_bytes": 20_000 });
    let id = app.watch(&token, "Steady", &room, config).await;

    room.wait_for_media_connection(1).await;
    let session_id = only_session(&app, &token, &id).await;
    room.reset();

    // The second connection records a whole part (about 38 KB of media
    // against a 20 KB limit) before it drops too.
    room.wait_for_media_connection(2).await;
    // A plain read, not `eventually`: the drop's NOT_LIVE is written before
    // the re-check that resumes, and a later part sets LIVE too, so only a
    // read right after the reconnect pins what the resume wrote.
    assert_eq!(app.streamer(&token, &id).await["state"], "LIVE");
    let mut flv = FlvLoop::tiny();
    room.push(flv.stream(6));
    eventually(WAIT, "a recorded part", || async {
        (segment_count(&app, &token, &session_id).await > 0).then_some(())
    })
    .await;
    room.reset();

    room.wait_for_media_connection(3).await;
    eventually(WAIT, "the streamer to read LIVE", || async {
        (app.streamer(&token, &id).await["state"] == "LIVE").then_some(())
    })
    .await;
    let session = app.session(&token, &session_id).await;
    assert_eq!(session["is_live"], true, "{session}");
    assert_eq!(end_payloads(&session), Vec::<&Value>::new());

    room.reset();
    let session = ended(&app, &token, &session_id).await;
    assert_eq!(end_payloads(&session), [&consecutive_failures_end()]);
    assert_eq!(app.sessions(&token, &id).await.len(), 1);

    app.shutdown().await;
}

/// Wait for the streamer's newest session to be a new one after `count`
/// sessions, and return its id.
async fn new_session(app: &TestApp, token: &str, streamer_id: &str, count: usize) -> String {
    eventually(WAIT, "a new session", || async {
        let sessions = app.sessions(token, streamer_id).await;
        (sessions.len() > count).then(|| sessions[0]["id"].as_str().expect("session id").to_owned())
    })
    .await
}

/// Disabling a streamer stops only its current recording: a session an
/// offline check ended keeps that cause, and once a new broadcast has opened
/// a second session, only the second ends as a user stop.
///
/// The offline check comes well inside the quiet period after the body
/// ends (4 s against 8 s); a quiet period that ran out first would end the
/// session as `completed`, a cause a disable soon after may take over.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_ends_only_the_current_session_as_a_user_stop() {
    let fake = FakePlatform::start().await;
    let over = fake.room("over");
    over.set_live(over.live("Only broadcast"));
    let again = fake.room("again");
    again.set_live(again.live("First broadcast"));
    let app = TestApp::start_with(Seed::live().global("offline_check_delay_ms", 4_000)).await;
    let token = app.admin_token().await;
    let over_id = app.watch(&token, "Over", &over, Value::Null).await;
    let again_id = app.watch(&token, "Again", &again, Value::Null).await;

    let offline_end = end("streamer_offline", true);
    let mut first_sessions = Vec::new();
    for (id, room) in [(&over_id, &over), (&again_id, &again)] {
        room.wait_for_media_connection(1).await;
        let session_id = only_session(&app, &token, id).await;
        room.broadcast_ends(1);
        assert_eq!(
            end_payloads(&ended(&app, &token, &session_id).await),
            [&offline_end]
        );
        first_sessions.push(session_id);
    }
    again.set_live(again.live("Second broadcast"));
    again.wait_for_media_connection(2).await;
    let second = new_session(&app, &token, &again_id, 1).await;

    for id in [&over_id, &again_id] {
        let disabled = update(&app, &token, id, &json!({ "enabled": false })).await;
        assert_eq!(disabled["state"], "DISABLED");
    }
    let second = ended(&app, &token, &second).await;
    assert_eq!(end_payloads(&second), [&end("user_disabled", false)]);
    for session_id in &first_sessions {
        assert_eq!(
            end_payloads(&app.session(&token, session_id).await),
            [&offline_end],
            "{session_id}"
        );
    }
    assert_eq!(app.sessions(&token, &over_id).await.len(), 1);

    app.shutdown().await;
}

/// Only disabling clears a backoff: a rename or a priority change keeps the
/// failure count, the last error and the backoff, so the streamer is not
/// checked early. Once disabling has cleared them, re-enabling checks the
/// streamer at once rather than when the old backoff would have ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updates_keep_a_backoff_until_disabling_clears_it_and_re_enabling_checks_at_once() {
    let fake = FakePlatform::start().await;
    let room = fake.room("resting");
    let until = now_ms() + 86_400_000;
    let app = TestApp::start_with(Seed::live().rows(&seeded_streamer(
        "resting",
        &room,
        "TEMPORAL_DISABLED",
        &[
            ("consecutive_error_count", "3"),
            ("last_error", "'Extraction failed (http)'"),
            ("disabled_until", &until.to_string()),
        ],
    )))
    .await;
    let token = app.admin_token().await;

    for change in [json!({ "name": "Renamed" }), json!({ "priority": "HIGH" })] {
        let updated = update(&app, &token, "resting", &change).await;
        let stored = app.streamer(&token, "resting").await;
        for streamer in [&updated, &stored] {
            let context = format!("{change}: {streamer}");
            assert_eq!(streamer["state"], "TEMPORAL_DISABLED", "{context}");
            assert_eq!(streamer["consecutive_error_count"], 3, "{context}");
            assert_eq!(
                streamer["last_error"], "Extraction failed (http)",
                "{change}"
            );
            assert_eq!(millis(&streamer["disabled_until"]), until, "{context}");
        }
    }
    assert_eq!(app.streamer(&token, "resting").await["name"], "Renamed");

    let disabled = update(&app, &token, "resting", &json!({ "enabled": false })).await;
    assert_eq!(disabled["state"], "DISABLED", "{disabled}");
    assert_eq!(disabled["consecutive_error_count"], 0, "{disabled}");
    assert!(disabled["last_error"].is_null(), "{disabled}");
    assert!(disabled["disabled_until"].is_null(), "{disabled}");
    let enabled = update(&app, &token, "resting", &json!({ "enabled": true })).await;
    assert_eq!(enabled["state"], "NOT_LIVE", "{enabled}");
    let history = eventually(WAIT, "a check after re-enabling", || async {
        let history = app.checks(&token, "resting").await;
        (!history.is_empty()).then_some(history)
    })
    .await;
    assert_eq!(history[0]["outcome"], "offline", "{history:?}");

    app.shutdown().await;
}

/// A streamer's URL is reserved ignoring case. A refused URL change keeps
/// both streamers' URLs reserved; an accepted one frees the old URL and
/// reserves the new one.
#[tokio::test]
async fn a_url_change_frees_the_old_url_and_reserves_the_new_one() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let create = async |name: &str, url: &str| {
        app.post(
            "/api/streamers",
            &token,
            &json!({ "name": name, "url": url, "enabled": false }),
        )
        .await
    };
    let moving = id_of(
        &create("Moving", "https://live.bilibili.com/7001")
            .await
            .created(),
    );
    create("Other", "https://live.bilibili.com/7002")
        .await
        .expect_status(StatusCode::CREATED);
    let path = format!("/api/streamers/{moving}");

    app.put(
        &path,
        &token,
        &json!({ "url": "https://LIVE.bilibili.com/7002" }),
    )
    .await
    .assert_error(StatusCode::CONFLICT, "CONFLICT");
    for url in [
        "https://LIVE.BILIBILI.COM/7001",
        "https://LIVE.BILIBILI.COM/7002",
    ] {
        create("Copy", url)
            .await
            .assert_error(StatusCode::CONFLICT, "CONFLICT");
    }

    let moved = json!({ "url": "https://live.bilibili.com/7003" });
    assert_eq!(
        update(&app, &token, &moving, &moved).await["url"],
        moved["url"]
    );
    for url in [
        "https://LIVE.BILIBILI.COM/7003",
        "https://LIVE.BILIBILI.COM/7002",
    ] {
        create("Copy", url)
            .await
            .assert_error(StatusCode::CONFLICT, "CONFLICT");
    }
    create("Successor", "https://live.bilibili.com/7001")
        .await
        .expect_status(StatusCode::CREATED);

    app.shutdown().await;
}

async fn segment_count(app: &TestApp, token: &str, session_id: &str) -> usize {
    app.segments(token, session_id).await.len()
}

/// How many files are under `dir`, recursively.
fn file_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("read a directory")
        .map(|entry| {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() { file_count(&path) } else { 1 }
        })
        .sum()
}

async fn streamer_rows(app: &TestApp, id: &str) -> i64 {
    app.query_i64(&format!("SELECT COUNT(*) FROM streamers WHERE id = '{id}'"))
        .await
}

/// Deleting a recording streamer, through its own route or a batch, ends
/// its session as a user stop and keeps the recording: the session, its
/// name, its part and its output stay, and the streamer is gone for a
/// second delete. Whether the row is removed inside the request depends on
/// how fast the session's end settles; see
/// `a_deleted_streamer_is_reaped_once_its_sessions_have_settled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_recording_streamer_ends_its_session_and_keeps_its_recording() {
    let fake = FakePlatform::start().await;
    let single = fake.room("single");
    single.set_live(single.live("Deleted alone"));
    let batched = fake.room("batched");
    batched.set_live(batched.live("Deleted in a batch"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    let single_id = app.watch(&token, "Single", &single, Value::Null).await;
    let batched_id = app.watch(&token, "Batched", &batched, Value::Null).await;
    let mut recordings = Vec::new();
    for (id, room) in [(&single_id, &single), (&batched_id, &batched)] {
        room.wait_for_media_connection(1).await;
        room.push(FlvLoop::tiny().stream(2));
        recordings.push(only_session(&app, &token, id).await);
    }
    // Both recordings have opened their part, so the stop finalizes one each.
    eventually(WAIT, "both recordings to open a part", || async {
        (file_count(app.output_dir()) >= 2).then_some(())
    })
    .await;

    let deleted = app
        .delete(&format!("/api/streamers/{single_id}"), &token)
        .await
        .ok();
    assert_eq!(deleted["success"], true);
    let batch = app
        .post(
            "/api/streamers/batch",
            &token,
            &json!({ "ids": [batched_id], "action": { "type": "delete" } }),
        )
        .await
        .ok();
    assert_eq!(batch["succeeded"], 1, "{batch}");

    for ((id, name), session_id) in [(&single_id, "Single"), (&batched_id, "Batched")]
        .into_iter()
        .zip(&recordings)
    {
        for response in [
            app.get(&format!("/api/streamers/{id}"), &token).await,
            app.delete(&format!("/api/streamers/{id}"), &token).await,
        ] {
            response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
        }
        let session = ended(&app, &token, session_id).await;
        let ends = end_payloads(&session);
        assert_eq!(ends.len(), 1, "{name}: {session}");
        assert_eq!(
            ends[0]["cause"],
            json!({ "type": "user_disabled" }),
            "{name}"
        );
        assert_eq!(session["streamer_name"], name);
        let session = eventually(WAIT, "the recorded part to be kept", || async {
            let session = app.session(&token, session_id).await;
            (session["output_count"] == 1).then_some(session)
        })
        .await;
        assert_eq!(segment_count(&app, &token, session_id).await, 1, "{name}");
        assert!(session["total_size_bytes"].as_u64() > Some(0), "{session}");
    }

    app.shutdown().await;
}

/// Deleting a streamer whose sessions have settled their final work removes
/// its row inside the request: its URL is free again and its recordings stay,
/// unowned, under the name they were recorded with. One whose ended session
/// still owes its final work stays marked, and is gone for a second delete
/// meanwhile.
#[tokio::test]
async fn a_deleted_streamer_is_reaped_once_its_sessions_have_settled() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let settled_url = "https://live.bilibili.com/8001";
    let settled = app.disabled_streamer(&token, "Settled", settled_url).await;
    let retiring = app
        .disabled_streamer(&token, "Retiring", "https://live.bilibili.com/8002")
        .await;
    // Inserted after startup, so the startup recovery does not settle the
    // second session.
    let (started, ended_at) = (now_ms() - 120_000, now_ms() - 60_000);
    app.run_sql(&format!(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             total_size_bytes, session_complete_dispatched) VALUES \
         ('settled-session', '{settled}', 'Settled', {started}, {ended_at}, 1000, 1), \
         ('unsettled-session', '{retiring}', 'Retiring', {started}, {ended_at}, 1024, 0);\n\
         INSERT INTO media_outputs (id, session_id, file_path, file_type, size_bytes, created_at) \
         VALUES ('settled-output', 'settled-session', '/recordings/settled-session.flv', 'VIDEO', \
             1000, {ended_at});\n{}",
        recorded_part("settled-session"),
    ))
    .await;

    app.delete(&format!("/api/streamers/{settled}"), &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(streamer_rows(&app, &settled).await, 0);
    let kept = app.session(&token, "settled-session").await;
    assert!(kept["streamer_id"].is_null(), "{kept}");
    assert_eq!(kept["streamer_name"], "Settled");
    assert_eq!(kept["output_count"], 1);
    assert_eq!(segment_count(&app, &token, "settled-session").await, 1);
    app.disabled_streamer(&token, "Successor", settled_url)
        .await;

    let path = format!("/api/streamers/{retiring}");
    app.delete(&path, &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(streamer_rows(&app, &retiring).await, 1);
    app.delete(&path, &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(streamer_rows(&app, &retiring).await, 1);

    app.shutdown().await;
}

// Generated by the crate's build script; `api::proto` is not exported. Not
// every generated message and accessor is used here.
#[allow(dead_code)]
mod download_progress {
    include!(concat!(env!("OUT_DIR"), "/download_progress.rs"));
}

/// The ids of the streamers waiting on the download queue, in arrival
/// order, from the snapshot a new downloads socket receives.
async fn queued(app: &TestApp, token: &str) -> Vec<String> {
    let mut socket = app
        .connect_socket(&with_query("/api/downloads/ws", &[("token", token)]), None)
        .await;
    let snapshot = tokio::time::timeout(WAIT, async {
        loop {
            let frame = match socket.next().await {
                Some(Ok(Message::Binary(frame))) => frame,
                Some(Ok(Message::Close(_))) | None => panic!("socket closed"),
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("socket failed: {error}"),
            };
            let message =
                download_progress::WsMessage::decode(frame).expect("decode a downloads frame");
            if let Some(download_progress::ws_message::Payload::Snapshot(snapshot)) =
                message.payload
            {
                return snapshot;
            }
        }
    })
    .await
    .expect("timed out waiting for a snapshot");
    snapshot
        .queued
        .into_iter()
        .map(|entry| entry.streamer_id)
        .collect()
}

async fn wait_until_queued(app: &TestApp, token: &str, id: &str) {
    eventually(WAIT, &format!("{id} to be queued"), || async {
        queued(app, token)
            .await
            .iter()
            .any(|queued| queued == id)
            .then_some(())
    })
    .await;
}

/// Create an enabled streamer of `priority` watching `room`, and return its id.
async fn watch_at(app: &TestApp, token: &str, room: &FakeRoom, priority: &str) -> String {
    let streamer = json!({ "name": room.url(), "url": room.url(), "priority": priority });
    id_of(&app.create_enabled_streamer(token, streamer).await)
}

/// A room per name, each live with its name as the title.
fn live_rooms<const N: usize>(fake: &FakePlatform, names: [&str; N]) -> [FakeRoom; N] {
    names.map(|name| {
        let room = fake.room(name);
        room.set_live(room.live(name));
        room
    })
}

fn recordings_started(fake: &FakePlatform, room: &FakeRoom) -> usize {
    fake.requests_to(room, "live.flv").len()
}

/// With every slot taken (one normal, plus two only high-priority
/// recordings may use), a freed slot goes to a waiting high-priority
/// recording before normal ones that waited longer; normal ones start in
/// arrival order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_recordings_start_high_priority_first_then_in_arrival_order() {
    let fake = FakePlatform::start().await;
    let [running, high_1, high_2, normal_1, normal_2, high_3] = &live_rooms(
        &fake,
        [
            "running", "high-1", "high-2", "normal-1", "normal-2", "high-3",
        ],
    );
    let app = TestApp::start_with(Seed::live().global("max_concurrent_downloads", 1)).await;
    let token = app.admin_token().await;

    for (room, priority) in [(running, "NORMAL"), (high_1, "HIGH"), (high_2, "HIGH")] {
        watch_at(&app, &token, room, priority).await;
        room.wait_for_media_connection(1).await;
    }
    let mut waiting = Vec::new();
    for (room, priority) in [(normal_1, "NORMAL"), (normal_2, "NORMAL"), (high_3, "HIGH")] {
        let id = watch_at(&app, &token, room, priority).await;
        wait_until_queued(&app, &token, &id).await;
        waiting.push(id);
    }
    assert_eq!(queued(&app, &token).await, waiting);

    running.broadcast_ends(1);
    high_3.wait_for_media_connection(1).await;
    assert_eq!(queued(&app, &token).await, waiting[..2]);

    high_3.broadcast_ends(1);
    normal_1.wait_for_media_connection(1).await;
    assert_eq!(queued(&app, &token).await, waiting[1..2]);
    assert_eq!(recordings_started(&fake, normal_2), 0);

    normal_1.broadcast_ends(1);
    normal_2.wait_for_media_connection(1).await;
    assert_eq!(queued(&app, &token).await, Vec::<String>::new());

    app.shutdown().await;
}

/// `max_concurrent_downloads` applies at runtime: raising it starts a
/// waiting recording at once; lowering it stops none, but new recordings
/// wait until the running ones are below the new limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_concurrent_downloads_changes_apply_to_waiting_recordings_at_once() {
    let fake = FakePlatform::start().await;
    let [first, second, third] = &live_rooms(&fake, ["first", "second", "third"]);
    let app = TestApp::start_with(Seed::live().global("max_concurrent_downloads", 1)).await;
    let token = app.admin_token().await;
    let set_limit = async |limit: u32| {
        app.patch(
            "/api/config/global",
            &token,
            &json!({ "max_concurrent_downloads": limit }),
        )
        .await
        .expect_status(StatusCode::OK);
    };

    let first_id = watch_at(&app, &token, first, "NORMAL").await;
    first.wait_for_media_connection(1).await;
    let second_id = watch_at(&app, &token, second, "NORMAL").await;
    wait_until_queued(&app, &token, &second_id).await;

    set_limit(2).await;
    second.wait_for_media_connection(1).await;

    set_limit(1).await;
    let third_id = watch_at(&app, &token, third, "NORMAL").await;
    wait_until_queued(&app, &token, &third_id).await;
    assert!(!first.media_closed() && !second.media_closed());

    // One recording ending leaves one running, which is the new limit.
    let first_session = only_session(&app, &token, &first_id).await;
    first.broadcast_ends(1);
    ended(&app, &token, &first_session).await;
    assert_eq!(queued(&app, &token).await, [third_id]);
    assert_eq!(recordings_started(&fake, third), 0);

    second.broadcast_ends(1);
    third.wait_for_media_connection(1).await;

    app.shutdown().await;
}

/// The offline notification of a session a restart left open reports how
/// long it lasted from its start before the restart, whether the first check
/// ends it or reuses it until the room goes offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_offline_notification_of_a_session_a_restart_left_open_counts_from_its_start() {
    let fake = FakePlatform::start().await;
    let gone = fake.room("gone");
    let back = fake.room("back");
    back.set_live(back.live("Before the restart"));
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app = TestApp::start_with(Seed::live().rows(&format!(
        "{}{}{}{}{}",
        seeded_streamer("gone", &gone, "LIVE", &[]),
        open_session("gone-session", "gone", "Before the restart"),
        seeded_streamer("back", &back, "LIVE", &[]),
        open_session("back-session", "back", "Before the restart"),
        webhook_channel(&receiver, &["stream_offline"]),
    )))
    .await;
    let token = app.admin_token().await;

    back.wait_for_media_connection(1).await;
    back.push(FlvLoop::tiny().stream(1));
    back.set_offline();
    back.end();
    let mut ended_sessions = Vec::new();
    for id in ["gone", "back"] {
        ended_sessions.push((id, ended(&app, &token, &format!("{id}-session")).await));
    }
    let received = eventually(WAIT, "both offline notifications", || async {
        let received = receiver.received();
        (received.len() >= 2).then_some(received)
    })
    .await;

    for (id, session) in ended_sessions {
        let body = received
            .iter()
            .map(ReceivedRequest::json)
            .find(|body| body["streamer_id"] == id)
            .unwrap_or_else(|| panic!("no offline notification for {id}"));
        let duration = body["data"]["StreamOffline"]["duration_secs"]
            .as_f64()
            .unwrap_or_else(|| panic!("no duration: {body}"));
        let lasted = millis(&session["end_time"]) - millis(&session["start_time"]);
        assert!(lasted >= 600_000, "{id}: {session}");
        assert!(
            ((duration * 1000.0).round() as i64 - lasted).abs() <= 1,
            "{id}: {duration} s for a session of {lasted} ms"
        );
    }

    app.shutdown().await;
}
