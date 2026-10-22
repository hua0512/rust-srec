//! What a recording on the fake platform resolves from its configuration
//! layers and its platform: output paths built from hostile titles and
//! names, the folder, engine and workflow a template or streamer overrides,
//! the stream every selection layer narrows down, and the cookies of the
//! selected account.

use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{FakePlatform, FakeRoom, FlvLoop, Seed, TestApp, WAIT, eventually, id_of};

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

/// `value` as an SQL string literal.
fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Create an enabled streamer watching `room` and return its id.
async fn watch(app: &TestApp, token: &str, room: &FakeRoom, body: Value) -> String {
    let mut body = body;
    body["url"] = json!(room.url());
    id_of(&app.create_enabled_streamer(token, body).await)
}

/// Record one repetition of the fixture in `room`, then let the room go
/// offline.
async fn broadcast_once(room: &FakeRoom) {
    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(1));
    room.set_offline();
    room.end();
}

/// Wait for the streamer's only session to end and return its id.
async fn ended_session(app: &TestApp, token: &str, streamer_id: &str) -> String {
    eventually(WAIT, "the session to end", || async {
        let sessions = get_ok(
            app,
            token,
            &format!("/api/sessions?streamer_id={streamer_id}&include_empty=true"),
        )
        .await["items"]
            .as_array()
            .expect("session items")
            .clone();
        assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
        sessions
            .first()
            .filter(|session| session["is_live"] == false)
            .map(id_of)
    })
    .await
}

/// The session's only segment file.
async fn segment_file(app: &TestApp, token: &str, session_id: &str) -> PathBuf {
    let segments = get_ok(app, token, &format!("/api/sessions/{session_id}/segments")).await;
    let items = segments["items"].as_array().expect("segment items");
    assert_eq!(items.len(), 1, "{segments}");
    PathBuf::from(items[0]["file_path"].as_str().expect("segment file path"))
}

/// Wait for the session's workflows to finish and return their names,
/// sorted.
async fn finished_workflows(
    app: &TestApp,
    token: &str,
    session_id: &str,
    count: usize,
) -> Vec<String> {
    let mut names: Vec<String> = eventually(WAIT, "the session's workflows to finish", || async {
        let dags = get_ok(
            app,
            token,
            &format!("/api/pipeline/dags?session_id={session_id}"),
        )
        .await["dags"]
            .as_array()
            .expect("dags")
            .clone();
        assert!(dags.len() <= count, "unexpected workflows: {dags:?}");
        (dags.len() == count && dags.iter().all(|dag| dag["status"] == "COMPLETED")).then_some(dags)
    })
    .await
    .iter()
    .map(|dag| dag["name"].as_str().expect("workflow name").to_owned())
    .collect();
    names.sort_unstable();
    names
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("a UTF-8 file name")
}

/// Panics unless `name` is `{prefix}-YYYYMMDD-HHMMSS-{suffix}`, the default
/// file name template.
#[track_caller]
fn assert_file_name(name: &str, prefix: &str, suffix: &str) {
    let stamp = name
        .strip_prefix(&format!("{prefix}-"))
        .and_then(|rest| rest.strip_suffix(&format!("-{suffix}")))
        .unwrap_or_else(|| panic!("{name:?} is not {prefix:?}-<time>-{suffix:?}"));
    assert!(
        stamp.len() == 15
            && stamp.char_indices().all(|(index, c)| if index == 8 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }),
        "{name:?}"
    );
}

/// Which characters are replaced, trimmed or prefixed is pinned by the unit
/// tests of `utils::filename`; this pins that a recording's folder, file and
/// workflow destination all go through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_titles_and_names_become_literal_safe_path_components() {
    let fake = FakePlatform::start().await;
    let room = fake.room("hostile");
    // Outer spaces and dots, a colon, a control character and a percent
    // token, which must stay literal.
    room.set_live(room.live(" ..Top: 5%d\u{1}end.. "));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let copied = app.output_dir().join("copied");
    let destination = copied
        .join("{platform}")
        .join("{streamer_id}")
        .join("{session_id}")
        .join("{streamer}")
        .join("{title}");
    let folder = app.output_dir().join("{streamer}").join("{title}");
    let id = watch(
        &app,
        &token,
        &room,
        json!({
            "name": "Streamer<Name>",
            "streamer_specific_config": {
                "output_folder": folder.to_string_lossy(),
                "session_complete_pipeline": {
                    "name": "copy-out",
                    "steps": [{
                        "id": "copy",
                        "step": {
                            "type": "inline",
                            "processor": "copy_move",
                            "config": {
                                "operation": "copy",
                                "destination": destination.to_string_lossy(),
                            },
                        },
                        "depends_on": [],
                    }],
                },
            },
        }),
    )
    .await;

    broadcast_once(&room).await;
    let session_id = ended_session(&app, &token, &id).await;
    let file = segment_file(&app, &token, &session_id).await;
    let (name, title) = ("Streamer_Name_", "Top_ 5%d_end");
    assert_eq!(
        file.parent(),
        Some(app.output_dir().join(name).join(title).as_path())
    );
    assert_file_name(file_name(&file), name, &format!("{title}.flv"));

    // The copy's destination fills in the session's identity and the same
    // components.
    assert_eq!(
        finished_workflows(&app, &token, &session_id, 1).await,
        ["copy-out"]
    );
    let jobs = get_ok(
        &app,
        &token,
        &format!("/api/pipeline/jobs?session_id={session_id}"),
    )
    .await["items"]
        .as_array()
        .expect("job items")
        .clone();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let copy = copied
        .join("fake")
        .join(&id)
        .join(&session_id)
        .join(name)
        .join(title)
        .join(file_name(&file));
    let outputs: Vec<&Path> = jobs[0]["output_path"]
        .as_array()
        .expect("job outputs")
        .iter()
        .map(|path| Path::new(path.as_str().expect("an output path")))
        .collect();
    assert_eq!(outputs, [copy.as_path()]);
    assert!(copy.is_file(), "{copy:?}");

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_folder_and_engine_and_a_streamer_workflow_override_the_global_ones() {
    let fake = FakePlatform::start().await;
    let own = fake.room("own-workflow");
    own.set_live(own.live("Own workflow"));
    let inherited = fake.room("inherited-workflow");
    inherited.set_live(inherited.live("Inherited workflow"));
    // The global engine cannot record: its binary does not exist.
    let missing_ffmpeg = json!({
        "binary_path": "/nonexistent/e2e-ffmpeg",
        "input_args": [],
        "output_args": [],
        "timeout_secs": 30,
        "user_agent": null,
    });
    let app = TestApp::start_with(
        Seed::live()
            .global("default_download_engine", sql_text("e2e-missing-ffmpeg"))
            .global("pipeline", sql_text(&succeed("global-segment").to_string()))
            .rows(&format!(
                "INSERT INTO engine_configuration (id, name, engine_type, config) \
                 VALUES ('e2e-missing-ffmpeg', 'e2e-missing-ffmpeg', 'FFMPEG', {});\n",
                sql_text(&missing_ffmpeg.to_string())
            )),
    )
    .await;
    let token = app.admin_token().await;
    let folder = app.output_dir().join("template-folder");
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Archive",
                "output_folder": folder.to_string_lossy(),
                "download_engine": "default-mesio",
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let template_id = id_of(&template);

    let own_id = watch(
        &app,
        &token,
        &own,
        json!({
            "name": "Own",
            "template_id": template_id,
            "streamer_specific_config": { "pipeline": succeed("streamer-segment") },
        }),
    )
    .await;
    let inherited_id = watch(
        &app,
        &token,
        &inherited,
        json!({ "name": "Inherited", "template_id": template_id }),
    )
    .await;

    for room in [&own, &inherited] {
        broadcast_once(room).await;
    }
    for (id, workflow) in [
        (&own_id, "streamer-segment"),
        (&inherited_id, "global-segment"),
    ] {
        let session_id = ended_session(&app, &token, id).await;
        let file = segment_file(&app, &token, &session_id).await;
        assert_eq!(file.parent(), Some(folder.as_path()), "{workflow}");
        assert_eq!(
            finished_workflows(&app, &token, &session_id, 1).await,
            [workflow]
        );
    }

    app.shutdown().await;
}

/// A stream of `room` at its FLV endpoint, told apart by the `edge` in its
/// query. Like the platforms that name no CDN, it carries no `cdn` extra, so
/// CDN settings match its URL.
fn edge_stream(room: &FakeRoom, edge: &str, bitrate: u64) -> Value {
    json!({
        "url": format!("{}?edge={edge}", room.flv_url()),
        "format": "flv",
        "media": "flv",
        "quality": "origin",
        "bitrate": bitrate,
    })
}

/// How the layers merge each selection field is pinned by the unit tests of
/// `downloader::stream_selector`; this pins that every layer reaches the
/// selection, and that the request for the selected stream carries the
/// cookies of the account the platform selects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_selection_layer_and_the_selected_accounts_cookies_reach_the_media_request() {
    let fake = FakePlatform::start().await;
    let room = fake.room("layered");
    // Equal but for bitrate: one stream per layer's blacklist ranks first,
    // then the expected pick, then the room's own stream at 16 kbps.
    let mut live = room.live("Layered");
    for (edge, bitrate) in [
        ("alpha", 30_000),
        ("beta", 30_000),
        ("gamma", 30_000),
        ("delta", 20_000),
    ] {
        live = live.stream(edge_stream(&room, edge, bitrate));
    }
    room.set_live(live);
    let platform = json!({ "blacklisted_cdns": ["alpha"] });
    let app = TestApp::start_with(
        Seed::live().platform("stream_selection_config", sql_text(&platform.to_string())),
    )
    .await;
    let token = app.admin_token().await;
    let profile = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-fake",
                "label": "Fake account",
                "enabled": true,
                "material": {
                    "cookies": "session=e2e; uid=42",
                    "refresh_token": null,
                    "access_token": null,
                    "reauth_config": null,
                },
            }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    let path = "/api/config/platforms/platform-fake";
    let mut platform = get_ok(&app, &token, path).await;
    // The platform route carries the selection as a JSON document in a string.
    platform["credential_selection"] =
        json!(json!({ "mode": "fixed", "credential_id": id_of(&profile) }).to_string());
    app.put(path, &token, &platform)
        .await
        .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Selective",
                "stream_selection_config": json!({ "blacklisted_cdns": ["beta"] }).to_string(),
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    watch(
        &app,
        &token,
        &room,
        json!({
            "name": "Layered",
            "template_id": id_of(&template),
            "streamer_specific_config": {
                "stream_selection_config": { "blacklisted_cdns": ["gamma"] },
            },
        }),
    )
    .await;

    room.wait_for_media_connection(1).await;
    let media = fake.requests_to(&room, "live.flv");
    assert_eq!(media.len(), 1, "{media:?}");
    assert!(media[0].path.ends_with("/live.flv?edge=delta"), "{media:?}");
    assert_eq!(media[0].header("cookie"), Some("session=e2e; uid=42"));

    app.shutdown().await;
}

/// A blacklisted CDN is an exclusion, not a preference: a room whose every
/// stream is on one is checked again rather than recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_offering_only_blacklisted_streams_is_not_recorded() {
    let fake = FakePlatform::start().await;
    let room = fake.room("blacklisted");
    room.set_live(room.live("Nowhere to go"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = watch(
        &app,
        &token,
        &room,
        json!({
            "name": "Blacklisted",
            "streamer_specific_config": {
                "stream_selection_config": { "blacklisted_cdns": ["__srec_fake__"] },
            },
        }),
    )
    .await;
    let checks = |count: usize| {
        let app = &app;
        let token = &token;
        let id = &id;
        async move {
            eventually(WAIT, &format!("{count} checks"), || async {
                let history =
                    get_ok(app, token, &format!("/api/streamers/{id}/check-history")).await;
                let items = history["items"].as_array().expect("check history").clone();
                (items.len() >= count).then_some(items)
            })
            .await
        }
    };

    let first = checks(1).await;
    assert_eq!(first[0]["outcome"], "offline", "{first:?}");
    // The streamer is checked again rather than recorded.
    checks(2).await;
    assert_eq!(fake.requests_to(&room, "live.flv").len(), 0);
    let sessions = get_ok(
        &app,
        &token,
        &format!("/api/sessions?streamer_id={id}&include_empty=true"),
    )
    .await;
    assert_eq!(sessions["items"], json!([]), "{sessions}");

    app.shutdown().await;
}
