//! Danmu recorded from the fake platform's rooms: the XML written beside each
//! video segment, its media output, and the paired-segment workflow that gets
//! both.
//!
//! Every app here is seeded with [`Seed::live`], whose fake platform row has
//! `record_danmu` off; streamers turn it on in their own config. The danmu
//! runner writes only into an open segment, so tests chat after the segment's
//! XML exists, and switch the room offline before ending its body (see
//! `recording.rs`).

use std::path::Path;

use serde_json::{Value, json};

use crate::support::fake_danmu::FakeDanmu;
use crate::support::{
    FakePlatform, FakeRoom, FlvLoop, Seed, TERMINAL_DAG_STATUSES, TestApp, WAIT, eventually,
    succeed,
};

/// Wait until the session has `count` media outputs and return them sorted by
/// file type.
async fn outputs(app: &TestApp, token: &str, session_id: &str, count: usize) -> Vec<Value> {
    let path = format!("/api/pipeline/outputs?session_id={session_id}");
    let mut outputs = eventually(WAIT, "the session's outputs", || async {
        let page = app.get_ok(&path, token).await;
        let items = page["items"].as_array().expect("output items").clone();
        assert!(items.len() <= count, "unexpected outputs: {items:?}");
        (items.len() == count).then_some(items)
    })
    .await;
    outputs.sort_by(|a, b| a["format"].as_str().cmp(&b["format"].as_str()));
    outputs
}

/// Every `.xml` file directly in `dir`.
fn xml_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read the output directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Watch the live `room` with `config` as the streamer's own config, record
/// three loops of FLV with `chat` sent while the segment's danmu XML is open,
/// and end the session. Returns the session id.
async fn record(
    app: &TestApp,
    token: &str,
    room: &FakeRoom,
    danmu: &FakeDanmu,
    config: Value,
    chat: &[(&str, &str)],
) -> String {
    let id = app.watch(token, "Chatty", room, config).await;
    room.wait_for_media_connection(1).await;
    if !chat.is_empty() {
        // The collector is registered before it connects, so once connected a
        // new segment opens its XML.
        danmu.wait_for_connection(1).await;
    }
    room.push(FlvLoop::tiny().stream(3));
    if !chat.is_empty() {
        eventually(WAIT, "the segment's danmu XML", || async {
            (!xml_files(app.output_dir()).is_empty()).then_some(())
        })
        .await;
        for (user, content) in chat {
            danmu.chat(user, content);
        }
    }
    room.set_offline();
    room.end();
    app.ended_session_id(token, &id).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn danmu_chat_is_recorded_beside_the_video_as_a_danmu_output() {
    let fake = FakePlatform::start().await;
    let room = fake.room("chatty");
    let danmu = FakeDanmu::of(&room);
    room.set_live(room.live("Chat night"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    let session_id = record(
        &app,
        &token,
        &room,
        &danmu,
        json!({ "record_danmu": true }),
        &[("viewer-1", "hello"), ("viewer-2", "fish & <chips>")],
    )
    .await;

    let outputs = outputs(&app, &token, &session_id, 2).await;
    assert_eq!(outputs[0]["format"], "DANMU_XML", "{outputs:?}");
    assert_eq!(outputs[1]["format"], "VIDEO", "{outputs:?}");
    let video = outputs[1]["file_path"].as_str().expect("video path");
    let xml = outputs[0]["file_path"].as_str().expect("danmu path");
    assert_eq!(
        Path::new(xml),
        Path::new(video).with_extension("xml"),
        "{outputs:?}"
    );
    assert_eq!(xml_files(app.output_dir()), [xml]);

    let contents = std::fs::read_to_string(xml).expect("read the danmu XML");
    assert_eq!(
        outputs[0]["file_size_bytes"],
        contents.len() as u64,
        "{outputs:?}"
    );
    assert!(contents.ends_with("</i>\n"), "{contents}");
    let chat: Vec<&str> = contents
        .lines()
        .filter(|line| line.trim_start().starts_with("<d "))
        .map(|line| line.split_once("\" user=").expect("a user attribute").1)
        .collect();
    assert_eq!(
        chat,
        [
            "\"viewer-1\">hello</d>",
            "\"viewer-2\">fish &amp; &lt;chips&gt;</d>"
        ],
        "{contents}"
    );
    assert_eq!(danmu.connections(), 1);

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_paired_segment_pipeline_receives_the_segments_video_and_danmu() {
    let fake = FakePlatform::start().await;
    let room = fake.room("paired");
    let danmu = FakeDanmu::of(&room);
    room.set_live(room.live("Paired"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    let session_id = record(
        &app,
        &token,
        &room,
        &danmu,
        json!({ "record_danmu": true, "paired_segment_pipeline": succeed("paired") }),
        &[("viewer-1", "hello")],
    )
    .await;

    let outputs = outputs(&app, &token, &session_id, 2).await;
    let xml = outputs[0]["file_path"].as_str().expect("danmu path");
    let video = outputs[1]["file_path"].as_str().expect("video path");

    let dag = eventually(WAIT, "the paired workflow to finish", || async {
        let dags = app.dags(&token, &session_id).await;
        assert!(dags.len() <= 1, "unexpected workflows: {dags:?}");
        dags.into_iter()
            .find(|dag| TERMINAL_DAG_STATUSES.contains(&dag["status"].as_str().unwrap_or_default()))
    })
    .await;
    assert_eq!(dag["name"], "paired", "{dag}");
    assert_eq!(dag["status"], "COMPLETED", "{dag}");

    let dag_id = dag["id"].as_str().expect("dag id");
    let jobs = app
        .get_ok(&format!("/api/pipeline/jobs?pipeline_id={dag_id}"), &token)
        .await;
    let jobs = jobs["items"].as_array().expect("job items");
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["input_path"], json!([video, xml]), "{jobs:?}");

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_record_danmu_no_danmu_is_collected_and_no_paired_pipeline_runs() {
    let fake = FakePlatform::start().await;
    let room = fake.room("quiet");
    let danmu = FakeDanmu::of(&room);
    room.set_live(room.live("Quiet"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    let session_id = record(
        &app,
        &token,
        &room,
        &danmu,
        json!({ "record_danmu": false, "paired_segment_pipeline": succeed("paired") }),
        &[],
    )
    .await;

    // The backend decides on the session-complete workflow only after the
    // segment's own work, so nothing is added for the session after it.
    let query =
        format!("SELECT session_complete_dispatched FROM live_sessions WHERE id = '{session_id}'");
    eventually(WAIT, "the session-complete decision", || async {
        (app.query_i64(&query).await == 1).then_some(())
    })
    .await;
    let outputs = outputs(&app, &token, &session_id, 1).await;
    assert_eq!(outputs[0]["format"], "VIDEO", "{outputs:?}");
    assert_eq!(xml_files(app.output_dir()), Vec::<String>::new());
    let dags = app
        .get_ok(
            &format!("/api/pipeline/dags?session_id={session_id}"),
            &token,
        )
        .await;
    assert_eq!(dags["dags"], json!([]), "{dags}");
    assert_eq!(danmu.connections(), 0);

    app.shutdown().await;
}
