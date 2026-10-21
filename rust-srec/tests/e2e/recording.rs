//! Recording live rooms on the fake platform: sessions, segments, outputs,
//! the files on disk and the session-complete workflow.
//!
//! Every app here is seeded with [`Seed::live`]. The fake platform's FLV body
//! ends without saying whether the broadcast did, so a clean end parks the
//! session until the next status check decides: offline ends it, live resumes
//! it. Tests therefore switch a room offline before ending its body.

use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{
    FakePlatform, FakeRoom, FlvLoop, Seed, TestApp, WAIT, eventually, id_of, millis, now_ms,
};

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// A workflow of one `execute` step that succeeds on every OS.
fn succeed(name: &str) -> Value {
    json!({
        "name": name,
        "steps": [{
            "id": "succeed",
            "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
            "depends_on": [],
        }],
    })
}

/// Create an enabled streamer watching `room`, with `config` as its
/// streamer-specific config, and return its id.
async fn watch(app: &TestApp, token: &str, name: &str, room: &FakeRoom, config: Value) -> String {
    let created = app
        .create_enabled_streamer(
            token,
            json!({ "name": name, "url": room.url(), "streamer_specific_config": config }),
        )
        .await;
    assert_eq!(created["platform_config_id"], "platform-fake", "{created}");
    id_of(&created)
}

async fn streamer(app: &TestApp, token: &str, id: &str) -> Value {
    get_ok(app, token, &format!("/api/streamers/{id}")).await
}

/// The streamer's sessions, newest first.
async fn sessions(app: &TestApp, token: &str, streamer_id: &str) -> Vec<Value> {
    let page = get_ok(
        app,
        token,
        &format!("/api/sessions?streamer_id={streamer_id}&include_empty=true"),
    )
    .await;
    page["items"].as_array().expect("session items").clone()
}

/// Wait for the streamer's only session to end and return its detail.
async fn ended_session(app: &TestApp, token: &str, streamer_id: &str) -> Value {
    let id = eventually(WAIT, "the session to end", || async {
        let sessions = sessions(app, token, streamer_id).await;
        assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
        sessions
            .first()
            .filter(|session| session["is_live"] == false)
            .map(|session| session["id"].as_str().expect("session id").to_owned())
    })
    .await;
    get_ok(app, token, &format!("/api/sessions/{id}")).await
}

fn session_id(session: &Value) -> &str {
    session["id"].as_str().expect("session id")
}

fn event_kinds(session: &Value) -> Vec<&str> {
    session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .map(|event| event["kind"].as_str().expect("event kind"))
        .collect()
}

/// The payload of the session's `session_ended` event.
fn end_payload(session: &Value) -> &Value {
    &session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .find(|event| event["kind"] == "session_ended")
        .unwrap_or_else(|| panic!("no session_ended event: {session}"))["payload"]
}

/// The session's segments, oldest first.
async fn segments(app: &TestApp, token: &str, session_id: &str) -> Vec<Value> {
    let page = get_ok(app, token, &format!("/api/sessions/{session_id}/segments")).await;
    let mut items = page["items"].as_array().expect("segment items").clone();
    items.sort_by_key(|segment| segment["segment_index"].as_i64());
    items
}

fn segment_path(segment: &Value) -> &str {
    segment["file_path"].as_str().expect("segment file path")
}

async fn outputs(app: &TestApp, token: &str, session_id: &str) -> Vec<Value> {
    let page = get_ok(
        app,
        token,
        &format!("/api/pipeline/outputs?session_id={session_id}"),
    )
    .await;
    page["items"].as_array().expect("output items").clone()
}

async fn dags(app: &TestApp, token: &str, session_id: &str) -> Vec<Value> {
    let page = get_ok(
        app,
        token,
        &format!("/api/pipeline/dags?session_id={session_id}"),
    )
    .await;
    page["dags"].as_array().expect("dags").clone()
}

/// Wait for the session's `count` workflows to finish and return them sorted
/// by name.
async fn finished_dags(app: &TestApp, token: &str, session_id: &str, count: usize) -> Vec<Value> {
    let mut finished = eventually(WAIT, "the session's workflows to finish", || async {
        let dags = dags(app, token, session_id).await;
        assert!(dags.len() <= count, "unexpected workflows: {dags:?}");
        (dags.len() == count
            && dags.iter().all(|dag| {
                ["COMPLETED", "FAILED", "CANCELLED"]
                    .contains(&dag["status"].as_str().unwrap_or_default())
            }))
        .then_some(dags)
    })
    .await;
    finished.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    finished
}

/// Wait until the backend has decided on the session's session-complete
/// workflow, whether it ran one or not; no workflow is added for the
/// session after that.
async fn wait_for_session_complete_decision(app: &TestApp, session_id: &str) {
    let query =
        format!("SELECT session_complete_dispatched FROM live_sessions WHERE id = '{session_id}'");
    eventually(WAIT, "the session-complete decision", || async {
        (app.query_i64(&query).await == 1).then_some(())
    })
    .await;
}

/// Every file under `dir`, recursively, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read a directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn file_size(path: &str) -> u64 {
    std::fs::metadata(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .len()
}

/// Panics unless `segment` describes an FLV file on disk of its recorded size.
#[track_caller]
fn assert_recorded(segment: &Value) {
    let path = segment_path(segment);
    let recorded = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(&recorded[..3], b"FLV", "{path}");
    assert_eq!(segment["size_bytes"], file_size(path), "{segment}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enabled_streamer_goes_live_records_flv_and_ends_when_the_platform_reports_offline() {
    let fake = FakePlatform::start().await;
    let room = fake.room("alice");
    room.set_live(
        room.live("Morning stream")
            .category("Talk")
            .viewers(42)
            .header("Referer", "https://fake.invalid/alice"),
    );
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("archive"))).await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Alice", &room, Value::Null).await;

    room.wait_for_media_connection(1).await;
    assert_eq!(streamer(&app, &token, &id).await["state"], "LIVE");
    room.push(FlvLoop::tiny().stream(3));
    room.set_offline();
    room.end();

    let session = ended_session(&app, &token, &id).await;
    assert_eq!(session["streamer_id"], id.as_str());
    assert_eq!(session["streamer_name"], "Alice");
    assert_eq!(session["title"], "Morning stream");
    assert_eq!(
        event_kinds(&session),
        ["session_started", "hysteresis_entered", "session_ended"]
    );
    assert_eq!(
        session["events"][0]["payload"],
        json!({ "kind": "session_started", "from_hysteresis": false, "title": "Morning stream" })
    );
    assert_eq!(
        session["events"][1]["payload"]["cause"],
        json!({ "type": "completed" })
    );
    assert_eq!(
        end_payload(&session),
        &json!({ "kind": "session_ended", "cause": { "type": "streamer_offline" }, "via_hysteresis": true })
    );

    // One segment, as one file directly in the output directory.
    let segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    let segment = &segments[0];
    assert_eq!(segment["segment_index"], 0);
    assert_eq!(segment["duration_secs"], 6.0);
    assert!(segment["split_reason_code"].is_null(), "{segment}");
    assert_recorded(segment);
    let path = segment_path(segment);
    let file = Path::new(path);
    assert_eq!(files_under(app.output_dir()), [file.to_path_buf()]);
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .expect("file name");
    assert!(
        name.starts_with("Alice-") && name.ends_with("-Morning stream.flv"),
        "{name}"
    );
    assert_eq!(session["output_count"], 1);
    assert_eq!(session["total_size_bytes"], file_size(path));

    let outputs = outputs(&app, &token, session_id(&session)).await;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    assert_eq!(outputs[0]["file_path"], path);
    assert_eq!(outputs[0]["format"], "VIDEO");
    assert_eq!(outputs[0]["file_size_bytes"], file_size(path));
    assert_eq!(outputs[0]["streamer_id"], id.as_str());

    let dags = finished_dags(&app, &token, session_id(&session), 1).await;
    assert_eq!(dags[0]["name"], "archive");
    assert_eq!(dags[0]["status"], "COMPLETED", "{dags:?}");
    assert_eq!(dags[0]["streamer_id"], id.as_str());

    let ended = streamer(&app, &token, &id).await;
    assert_eq!(ended["state"], "NOT_LIVE", "{ended}");
    assert_eq!(ended["consecutive_error_count"], 0);
    assert_eq!(
        millis(&ended["last_live_time"]),
        millis(&session["start_time"])
    );

    // The check that found the room live describes its candidates without
    // their URLs; every check after it found the room offline.
    let checks = eventually(WAIT, "the offline check in the history", || async {
        let history = get_ok(&app, &token, &format!("/api/streamers/{id}/check-history")).await;
        let items = history["items"].as_array().expect("check history").clone();
        (items.len() >= 2).then_some(items)
    })
    .await;
    let candidate = json!({
        "bitrate": 16_000, "codec": "", "fps": 0.0,
        "media_format": "Flv", "quality": "origin", "stream_format": "Flv",
    });
    let live = &checks[0];
    assert_eq!(live["outcome"], "live", "{checks:?}");
    assert_eq!(live["title"], "Morning stream");
    assert_eq!(live["category"], "Talk");
    assert_eq!(live["viewer_count"], 42);
    assert_eq!(live["streams_extracted"], 1);
    assert_eq!(live["stream_selected"], candidate);
    assert_eq!(live["streams_extracted_detail"], json!([candidate]));
    assert!(
        checks[1..]
            .iter()
            .all(|check| check["outcome"] == "offline"),
        "{checks:?}"
    );

    // Status checks carry the platform's own headers, the one media request
    // the headers the platform answered with, and nothing took the dead-end
    // route.
    for request in fake.requests_to(&room, "status") {
        assert_eq!(
            request.header("accept-language"),
            Some("zh-CN,zh;q=0.8,en-US;q=0.5,en;q=0.3"),
            "{request:?}"
        );
    }
    let media_requests = fake.requests_to(&room, "live.flv");
    assert_eq!(media_requests.len(), 1, "{media_requests:?}");
    assert_eq!(
        media_requests[0].header("referer"),
        Some("https://fake.invalid/alice")
    );
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_brief_disconnect_resumes_the_same_session() {
    let fake = FakePlatform::start().await;
    let room = fake.room("bob");
    room.set_live(room.live("Late show"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Bob", &room, Value::Null).await;

    let mut flv = FlvLoop::tiny();
    room.wait_for_media_connection(1).await;
    room.push(flv.stream(1));
    eventually(WAIT, "the first part to be opened", || async {
        (!files_under(app.output_dir()).is_empty()).then_some(())
    })
    .await;
    // The connection drops while the room stays live: the next check finds
    // it live and the recording reconnects.
    room.reset();
    room.wait_for_media_connection(2).await;
    room.push(flv.stream(1));
    room.set_offline();
    room.end();

    let session = ended_session(&app, &token, &id).await;
    assert_eq!(sessions(&app, &token, &id).await.len(), 1);
    assert_eq!(
        event_kinds(&session),
        [
            "session_started",
            "hysteresis_entered",
            "session_resumed",
            "session_started",
            "hysteresis_entered",
            "session_ended",
        ]
    );
    let events = &session["events"];
    assert_eq!(events[1]["payload"]["kind"], "hysteresis_entered");
    assert_eq!(
        events[1]["payload"]["cause"],
        json!({ "type": "failed", "kind": "network" })
    );
    assert_eq!(events[2]["payload"]["kind"], "session_resumed");
    assert_eq!(events[3]["payload"]["from_hysteresis"], true);
    assert_eq!(
        events[4]["payload"]["cause"],
        json!({ "type": "completed" })
    );
    assert_eq!(
        end_payload(&session)["cause"],
        json!({ "type": "streamer_offline" })
    );

    // Each connection left its own part; the second holds the whole
    // repetition sent after the reconnect.
    let segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(segments.len(), 2, "{segments:?}");
    assert_eq!(segments[0]["segment_index"], 0);
    assert_eq!(segments[1]["segment_index"], 1);
    assert_eq!(segments[1]["duration_secs"], 2.0);
    assert_ne!(segment_path(&segments[0]), segment_path(&segments[1]));
    for segment in &segments {
        assert_recorded(segment);
    }
    assert_eq!(files_under(app.output_dir()).len(), 2);
    assert_eq!(session["output_count"], 2);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_must_be_confirmed_offline_check_count_times_before_the_session_ends() {
    let fake = FakePlatform::start().await;
    let room = fake.room("carol");
    room.set_live(room.live("Confirmed"));
    let app = TestApp::start_with(Seed::live().global("offline_check_count", 3)).await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Carol", &room, Value::Null).await;

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();

    // The body ending cleanly is the first of the three offline signals, so
    // the session ends at the second offline check after it.
    let session = ended_session(&app, &token, &id).await;
    assert_eq!(
        end_payload(&session)["cause"],
        json!({ "type": "streamer_offline" })
    );
    // History rows are written in check order and after the check, so one
    // from after the end means every earlier row is in.
    let ended_at = millis(&session["end_time"]);
    let history = eventually(WAIT, "a check after the end", || async {
        let history = get_ok(&app, &token, &format!("/api/streamers/{id}/check-history")).await;
        let items = history["items"].as_array().expect("check history").clone();
        items
            .iter()
            .any(|check| millis(&check["checked_at"]) > ended_at)
            .then_some(items)
    })
    .await;
    let offline_before_end = history
        .iter()
        .filter(|check| check["outcome"] == "offline" && millis(&check["checked_at"]) <= ended_at)
        .count();
    assert_eq!(offline_before_end, 2, "{history:?}");

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_a_recording_streamer_ends_its_session_and_runs_the_session_complete_pipeline() {
    let fake = FakePlatform::start().await;
    let room = fake.room("dave");
    room.set_live(room.live("Interrupted"));
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("after-disable")))
            .await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Dave", &room, Value::Null).await;

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(2));
    eventually(WAIT, "the recording to be opened", || async {
        (!files_under(app.output_dir()).is_empty()).then_some(())
    })
    .await;
    let disabled = app
        .put(
            &format!("/api/streamers/{id}"),
            &token,
            &json!({ "enabled": false }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(disabled["state"], "DISABLED");

    // The room is still live, so the backend has to hang up by itself.
    eventually(WAIT, "the backend to hang up", || async {
        room.media_closed().then_some(())
    })
    .await;
    let session = ended_session(&app, &token, &id).await;
    assert_eq!(event_kinds(&session), ["session_started", "session_ended"]);
    assert_eq!(
        end_payload(&session),
        &json!({ "kind": "session_ended", "cause": { "type": "user_disabled" }, "via_hysteresis": false })
    );
    // The workflow starts once the session's parts are stored.
    let dags = finished_dags(&app, &token, session_id(&session), 1).await;
    assert_eq!(dags[0]["name"], "after-disable");
    assert_eq!(dags[0]["status"], "COMPLETED", "{dags:?}");
    let segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_recorded(&segments[0]);
    assert_eq!(streamer(&app, &token, &id).await["state"], "DISABLED");
    assert_eq!(fake.requests_to(&room, "live.flv").len(), 1);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn part_size_and_duration_limits_split_a_mesio_recording() {
    let fake = FakePlatform::start().await;
    let by_size = fake.room("sized");
    by_size.set_live(by_size.live("Sized"));
    let by_duration = fake.room("timed");
    by_duration.set_live(by_duration.live("Timed"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    // Six repetitions are about 38 KB of media tags spanning 12.7 s.
    let sized = watch(
        &app,
        &token,
        "Sized",
        &by_size,
        json!({ "max_part_size_bytes": 20_000 }),
    )
    .await;
    let timed = watch(
        &app,
        &token,
        "Timed",
        &by_duration,
        json!({ "max_download_duration_secs": 4 }),
    )
    .await;

    for room in [&by_size, &by_duration] {
        room.wait_for_media_connection(1).await;
        room.push(FlvLoop::tiny().stream(6));
        room.set_offline();
        room.end();
    }
    let parts = |segments: &[Value]| -> Vec<(Value, Value)> {
        segments
            .iter()
            .map(|segment| {
                assert_recorded(segment);
                (
                    segment["duration_secs"].clone(),
                    segment["split_reason_code"].clone(),
                )
            })
            .collect()
    };

    let session = ended_session(&app, &token, &sized).await;
    let sized_parts = segments(&app, &token, session_id(&session)).await;
    assert_eq!(
        parts(&sized_parts),
        [(json!(6.0), json!("size_limit")), (json!(6.0), Value::Null)],
        "{sized_parts:?}"
    );
    let session = ended_session(&app, &token, &timed).await;
    let timed_parts = segments(&app, &token, session_id(&session)).await;
    assert_eq!(
        parts(&timed_parts),
        [
            (json!(4.0), json!("duration_limit")),
            (json!(4.0), json!("duration_limit")),
            (json!(4.0), Value::Null),
        ],
        "{timed_parts:?}"
    );
    // The later parts of one download are numbered after the first.
    let first = segment_path(&timed_parts[0]).trim_end_matches(".flv");
    assert_eq!(segment_path(&timed_parts[1]), format!("{first}-001.flv"));
    assert_eq!(segment_path(&timed_parts[2]), format!("{first}-002.flv"));
    assert_eq!(files_under(app.output_dir()).len(), 5);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_titles_cannot_escape_the_output_directory() {
    let fake = FakePlatform::start().await;
    let room = fake.room("eve");
    // Both separators, parent references and a drive prefix.
    room.set_live(room.live(r"..\../C:/escape"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let folder = app.output_dir().join("{streamer}").join("{title}");
    let id = watch(
        &app,
        &token,
        "..",
        &room,
        json!({ "output_folder": folder.to_string_lossy() }),
    )
    .await;

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();
    let session = ended_session(&app, &token, &id).await;
    let segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_recorded(&segments[0]);

    // Each value became one path component: `..` alone becomes `unnamed`,
    // separators and the drive colon become `_`, and outer dots are trimmed.
    let file = PathBuf::from(segment_path(&segments[0]));
    let directory = app.output_dir().join("unnamed").join("_.._C_escape");
    assert_eq!(file.parent(), Some(directory.as_path()));
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .expect("file name");
    assert!(
        name.starts_with("unnamed-") && name.ends_with("-_.._C_escape.flv"),
        "{name}"
    );
    // Nothing was written beside the output directory but the database.
    let data_dir = app.output_dir().parent().expect("the data directory");
    let outside: Vec<PathBuf> = files_under(data_dir)
        .into_iter()
        .filter(|path| {
            !path.starts_with(app.output_dir())
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("srec.db"))
        })
        .collect();
    assert_eq!(outside, Vec::<PathBuf>::new());
    assert_eq!(files_under(app.output_dir()), [file]);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segments_smaller_than_the_minimum_are_discarded_and_not_pipelined() {
    let fake = FakePlatform::start().await;
    let short = fake.room("short");
    short.set_live(short.live("Too short"));
    let kept = fake.room("kept");
    kept.set_live(kept.live("Kept"));
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("session-complete")))
            .await;
    let token = app.admin_token().await;
    // The same workflows on both; only the minimum segment size differs.
    let per_segment = succeed("per-segment");
    let short_id = watch(
        &app,
        &token,
        "Short",
        &short,
        json!({ "min_segment_size_bytes": 10_000_000, "pipeline": per_segment }),
    )
    .await;
    let kept_id = watch(
        &app,
        &token,
        "Kept",
        &kept,
        json!({ "pipeline": per_segment }),
    )
    .await;

    for room in [&short, &kept] {
        room.wait_for_media_connection(1).await;
        room.push(FlvLoop::tiny().stream(1));
        room.set_offline();
        room.end();
    }

    let session = ended_session(&app, &token, &kept_id).await;
    let kept_segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(kept_segments.len(), 1, "{kept_segments:?}");
    let kept_dags = finished_dags(&app, &token, session_id(&session), 2).await;
    let names: Vec<&str> = kept_dags
        .iter()
        .map(|dag| dag["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, ["per-segment", "session-complete"]);

    let session = ended_session(&app, &token, &short_id).await;
    wait_for_session_complete_decision(&app, session_id(&session)).await;
    assert_eq!(
        segments(&app, &token, session_id(&session)).await,
        Vec::<Value>::new()
    );
    assert_eq!(
        outputs(&app, &token, session_id(&session)).await,
        Vec::<Value>::new()
    );
    assert_eq!(
        dags(&app, &token, session_id(&session)).await,
        Vec::<Value>::new()
    );
    assert_eq!(session["output_count"], 0);
    assert_eq!(session["total_size_bytes"], 0);
    // The discarded file is gone; the kept recording is the only one left.
    assert_eq!(
        files_under(app.output_dir()),
        [PathBuf::from(segment_path(&kept_segments[0]))]
    );

    app.shutdown().await;
}

/// A streamer row inserted before boot on `room`, for states the services
/// load at startup.
fn seeded_streamer(id: &str, room: &FakeRoom, columns: &[(&str, &str)]) -> String {
    let (names, values): (Vec<&str>, Vec<&str>) = columns.iter().copied().unzip();
    format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, priority{names}) \
         VALUES ('{id}', '{id}', '{url}', 'platform-fake', 'NORMAL'{values});\n",
        names = names
            .iter()
            .map(|name| format!(", {name}"))
            .collect::<String>(),
        values = values
            .iter()
            .map(|value| format!(", {value}"))
            .collect::<String>(),
        url = room.url(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mesio_config_saved_with_since_retired_settings_still_records() {
    let fake = FakePlatform::start().await;
    let room = fake.room("legacy");
    room.set_live(room.live("Legacy config"));
    // Output settings an older version stored on the default engine, which
    // this version no longer has.
    let stored = json!({
        "fix_flv": true,
        "fix_hls": true,
        "hls": { "output_config": {
            "live_reorder_buffer_duration_ms": 30_000,
            "live_reorder_buffer_max_segments": 10,
            "max_pending_init_segments": 4,
        } },
    });
    let app = TestApp::start_with(Seed::live().rows(&format!(
        "UPDATE engine_configuration SET config = '{stored}' WHERE id = 'default-mesio';"
    )))
    .await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Legacy", &room, Value::Null).await;

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();

    let session = ended_session(&app, &token, &id).await;
    let segments = segments(&app, &token, session_id(&session)).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_recorded(&segments[0]);

    app.shutdown().await;
}

/// After a restart the session a recording left open is in the database
/// only, not in memory. Disabling the streamer still ends it, as a user
/// stop; otherwise it would stay live forever and the next live check would
/// reuse it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_a_streamer_ends_the_session_a_restart_left_open() {
    let fake = FakePlatform::start().await;
    let room = fake.room("restarted");
    // Backing off for a day, so the scheduler does not check the room; were
    // it checked, an error keeps the check from ending the session itself.
    room.set_error("http", "unavailable");
    let backoff_until = now_ms() + 86_400_000;
    let started = now_ms() - 60_000;
    let app = TestApp::start_with(Seed::live().rows(&format!(
        "{}INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             total_size_bytes, session_complete_dispatched) \
         VALUES ('open-session', 'restarted', 'restarted', {started}, NULL, 0, 0);\n",
        seeded_streamer(
            "restarted",
            &room,
            &[
                ("state", "'TEMPORAL_DISABLED'"),
                ("disabled_until", &backoff_until.to_string()),
            ],
        ),
    )))
    .await;
    let token = app.admin_token().await;
    assert_eq!(
        sessions(&app, &token, "restarted").await[0]["is_live"],
        true
    );

    let disabled = app
        .put(
            "/api/streamers/restarted",
            &token,
            &json!({ "enabled": false }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(disabled["state"], "DISABLED");

    let session = ended_session(&app, &token, "restarted").await;
    assert_eq!(session_id(&session), "open-session");
    assert_eq!(
        end_payload(&session),
        &json!({ "kind": "session_ended", "cause": { "type": "user_disabled" }, "via_hysteresis": false })
    );

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamer_session_complete_pipeline_replaces_the_platform_one() {
    let fake = FakePlatform::start().await;
    let room = fake.room("frank");
    room.set_live(room.live("Own workflow"));
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("platform-level")))
            .await;
    let token = app.admin_token().await;
    let id = watch(
        &app,
        &token,
        "Frank",
        &room,
        json!({ "session_complete_pipeline": succeed("streamer-level") }),
    )
    .await;

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();

    let session = ended_session(&app, &token, &id).await;
    let dags = finished_dags(&app, &token, session_id(&session), 1).await;
    assert_eq!(dags[0]["name"], "streamer-level", "{dags:?}");
    assert_eq!(dags[0]["status"], "COMPLETED", "{dags:?}");

    app.shutdown().await;
}

/// Platforms that name no CDN in a stream's extras are matched against
/// preferred CDNs by URL; the preferred stream wins over one the platform
/// ranks higher.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preferred_cdns_match_the_stream_url_when_the_platform_names_no_cdn() {
    let fake = FakePlatform::start().await;
    let room = fake.room("grace");
    let preferred = format!("{}?edge=cloudfront", room.flv_url());
    room.set_live(room.live("Two edges").stream(json!({
        "url": preferred,
        "format": "flv",
        "media": "flv",
        "quality": "origin",
        "bitrate": 8_000,
        "priority": 1,
    })));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = watch(
        &app,
        &token,
        "Grace",
        &room,
        json!({ "stream_selection_config": { "preferred_cdns": ["cloudfront"] } }),
    )
    .await;

    room.wait_for_media_connection(1).await;
    let media = fake.requests_to(&room, "live.flv");
    assert_eq!(media.len(), 1, "{media:?}");
    assert!(
        media[0].path.ends_with("/live.flv?edge=cloudfront"),
        "{media:?}"
    );
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();
    ended_session(&app, &token, &id).await;

    app.shutdown().await;
}

/// Going live does not by itself clear a streamer's earlier failures; the
/// recording does. Its first part clears the last error, and the first
/// completed part clears the failure count too, through the committed
/// streamer row the API serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recording_clears_the_error_state_an_earlier_failure_left() {
    let fake = FakePlatform::start().await;
    let room = fake.room("henry");
    room.set_live(room.live("Recovered"));
    let app = TestApp::start_with(Seed::live().rows(&seeded_streamer(
        "henry",
        &room,
        &[
            ("state", "'NOT_LIVE'"),
            ("consecutive_error_count", "1"),
            ("last_error", "'Extraction failed (http)'"),
            (
                "streamer_specific_config",
                r#"'{"max_part_size_bytes":20000}'"#,
            ),
        ],
    )))
    .await;
    let token = app.admin_token().await;

    let mut flv = FlvLoop::tiny();
    room.wait_for_media_connection(1).await;
    room.push(flv.stream(1));
    let recording = eventually(WAIT, "the last error to clear", || async {
        let streamer = streamer(&app, &token, "henry").await;
        streamer["last_error"].is_null().then_some(streamer)
    })
    .await;
    assert_eq!(recording["state"], "LIVE", "{recording}");
    assert_eq!(recording["consecutive_error_count"], 1, "{recording}");

    // About 38 KB of media: the first part closes at the 20 KB limit.
    for _ in 0..5 {
        room.push(flv.next_loop());
    }
    let recovered = eventually(WAIT, "the failure count to clear", || async {
        let streamer = streamer(&app, &token, "henry").await;
        (streamer["consecutive_error_count"] == 0).then_some(streamer)
    })
    .await;
    assert_eq!(recovered["state"], "LIVE", "{recovered}");
    assert!(recovered["last_error"].is_null(), "{recovered}");
    assert!(recovered["disabled_until"].is_null(), "{recovered}");
    assert!(recovered["last_live_time"].is_string(), "{recovered}");

    room.set_offline();
    room.end();
    ended_session(&app, &token, "henry").await;

    app.shutdown().await;
}
