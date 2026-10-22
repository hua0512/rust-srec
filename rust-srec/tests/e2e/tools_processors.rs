//! Workflow processors running the real tools on real media: ffmpeg for
//! remux, metadata, audio extraction, subtitle burn-in and thumbnails, and
//! rclone for uploads.
//!
//! Each processor's argument building and output handling are pinned by its
//! unit tests with stand-in binaries; these scenarios check that the real
//! tools accept what the processors hand them and that the results reach
//! the API. They need ffmpeg, ffprobe and rclone on `PATH`, so they are
//! ignored in the plain suite; CI's tool-gated job runs them.

use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::tools::{codecs, duration_secs, ffmpeg, probe};
use crate::support::{FakePlatform, FlvLoop, Seed, TestApp, WAIT, eventually, id_of};

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

fn inline(id: &str, processor: &str, config: Value, depends_on: &[&str]) -> Value {
    json!({
        "id": id,
        "step": { "type": "inline", "processor": processor, "config": config },
        "depends_on": depends_on,
    })
}

/// A four-second FLV recording of H.264 and AAC, written to `relative`
/// under the app's output directory.
fn recording(app: &TestApp, relative: &str) -> String {
    app.write_output_file(relative, FlvLoop::tiny().stream(2))
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

/// Run a workflow of `steps` on `inputs` and return its detail once it has
/// settled.
async fn run_workflow(app: &TestApp, token: &str, inputs: &[&str], steps: Vec<Value>) -> Value {
    let created = app
        .post(
            "/api/pipeline/create",
            token,
            &json!({
                "session_id": "session-e2e",
                "streamer_id": "streamer-e2e",
                "input_paths": inputs,
                "dag": { "name": "tools", "steps": steps },
            }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    let dag_id = created.body["pipeline_id"]
        .as_str()
        .expect("pipeline_id")
        .to_owned();
    eventually(WAIT, &format!("workflow {dag_id} to settle"), || async {
        let dag = get_ok(app, token, &format!("/api/pipeline/dag/{dag_id}")).await;
        ["COMPLETED", "FAILED", "CANCELLED"]
            .contains(&dag["status"].as_str().unwrap_or_default())
            .then_some(dag)
    })
    .await
}

fn step<'a>(dag: &'a Value, step_id: &str) -> &'a Value {
    dag["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == step_id)
        .unwrap_or_else(|| panic!("step {step_id} in {dag}"))
}

/// Panics unless a [`probe`] result has H.264 and AAC streams lasting about
/// `secs` seconds (the fixture has one video frame a second).
#[track_caller]
fn assert_media(probed: &Value, secs: f64) {
    let mut found = codecs(probed);
    found.sort_unstable();
    assert_eq!(found, ["aac", "h264"], "{probed}");
    let duration = duration_secs(probed);
    assert!((duration - secs).abs() <= 1.0, "{duration} s: {probed}");
}

/// Wait for the streamer's only session to end and return its id.
async fn ended_session(app: &TestApp, token: &str, streamer_id: &str) -> String {
    let path = format!("/api/sessions?streamer_id={streamer_id}&include_empty=true");
    eventually(WAIT, "the session to end", || async {
        let page = get_ok(app, token, &path).await;
        let sessions = page["items"].as_array().expect("session items");
        assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
        sessions
            .first()
            .filter(|session| session["is_live"] == false)
            .map(|session| session["id"].as_str().expect("session id").to_owned())
    })
    .await
}

/// Record six seconds of the fixture from a fake room until the room
/// goes offline, returning the streamer's id and the session's.
async fn record(
    app: &TestApp,
    token: &str,
    fake: &FakePlatform,
    name: &str,
    config: Value,
) -> (String, String) {
    let room = fake.room(&name.to_lowercase());
    room.set_live(room.live("Processed"));
    let streamer = id_of(
        &app.create_enabled_streamer(
            token,
            json!({ "name": name, "url": room.url(), "streamer_specific_config": config }),
        )
        .await,
    );
    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(3));
    room.set_offline();
    room.end();
    let session = ended_session(app, token, &streamer).await;
    (streamer, session)
}

/// The session's workflows once they have all finished.
async fn finished_dags(app: &TestApp, token: &str, session_id: &str) -> Vec<Value> {
    let path = format!("/api/pipeline/dags?session_id={session_id}");
    eventually(WAIT, "the session's workflows to finish", || async {
        let page = get_ok(app, token, &path).await;
        let dags = page["dags"].as_array().expect("dags").clone();
        (!dags.is_empty()
            && dags.iter().all(|dag| {
                ["COMPLETED", "FAILED", "CANCELLED"]
                    .contains(&dag["status"].as_str().unwrap_or_default())
            }))
        .then_some(dags)
    })
    .await
}

/// A remux writes an MP4 next to the recording with the same streams, its
/// index moved to the front so playback can start before the download ends,
/// and removes the recording when asked to.
#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe"]
async fn remux_turns_a_flv_recording_into_a_faststart_mp4_and_removes_the_source() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = recording(&app, "rec/show.flv");

    let dag = run_workflow(
        &app,
        &token,
        &[&source],
        vec![inline(
            "remux",
            "remux",
            json!({ "remove_input_on_success": true }),
            &[],
        )],
    )
    .await;
    assert_eq!(dag["status"], "COMPLETED", "{dag}");
    let remuxed = app.output_path("rec/show.mp4");
    assert_eq!(step(&dag, "remux")["outputs"], json!([remuxed]));
    assert!(!exists(&source));

    let probed = probe(&remuxed).await;
    assert!(
        probed["format"]["format_name"]
            .as_str()
            .is_some_and(|name| name.contains("mp4")),
        "{probed}"
    );
    assert_media(&probed, 4.0);
    let bytes = read(&remuxed);
    let position = |atom: &[u8]| bytes.windows(4).position(|window| window == atom);
    let (moov, mdat) = (position(b"moov"), position(b"mdat"));
    assert!(
        moov.is_some() && mdat.is_some() && moov < mdat,
        "moov at {moov:?}, mdat at {mdat:?}"
    );

    app.shutdown().await;
}

/// Tags reach the file as given, including characters ffmpeg's option
/// syntax gives meaning to, while the streams are copied unchanged and the
/// input is kept by default.
#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe"]
async fn metadata_writes_tags_into_a_copy_of_the_recording() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let flv = recording(&app, "rec/tagged.flv");
    let source = app.output_path("rec/tagged.mp4");
    ffmpeg(&["-v", "error", "-i", &flv, "-c", "copy", &source]).await;
    let title = "Night: [live] 'special', a=b";

    let dag = run_workflow(
        &app,
        &token,
        &[&source],
        vec![inline(
            "tag",
            "metadata",
            json!({ "artist": "主播 Alice", "title": title, "comment": "100% live" }),
            &[],
        )],
    )
    .await;
    assert_eq!(dag["status"], "COMPLETED", "{dag}");
    let tagged = app.output_path("rec/tagged_meta.mp4");
    assert_eq!(step(&dag, "tag")["outputs"], json!([tagged]));
    assert!(exists(&source));

    let probed = probe(&tagged).await;
    assert_media(&probed, 4.0);
    let tags = &probed["format"]["tags"];
    assert_eq!(tags["artist"], "主播 Alice", "{probed}");
    assert_eq!(tags["title"], title, "{probed}");
    assert_eq!(tags["comment"], "100% live", "{probed}");

    app.shutdown().await;
}

/// Audio extraction names its output after the codec ffprobe finds and
/// copies the stream into it; a second extraction with `overwrite` off
/// fails on that output and leaves it, and the recording, as they were.
#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe"]
async fn audio_extract_with_overwrite_off_keeps_the_audio_an_earlier_run_extracted() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = recording(&app, "rec/voice.flv");
    let audio = app.output_path("rec/voice_audio.aac");

    let first = run_workflow(
        &app,
        &token,
        &[&source],
        vec![inline("audio", "audio_extract", json!({}), &[])],
    )
    .await;
    assert_eq!(first["status"], "COMPLETED", "{first}");
    assert_eq!(step(&first, "audio")["outputs"], json!([audio]));
    let probed = probe(&audio).await;
    assert_eq!(codecs(&probed), ["aac"], "{probed}");
    let extracted = read(&audio);

    let second = run_workflow(
        &app,
        &token,
        &[&source],
        vec![inline(
            "audio",
            "audio_extract",
            json!({ "overwrite": false }),
            &[],
        )],
    )
    .await;
    assert_eq!(second["status"], "FAILED", "{second}");
    let job_id = step(&second, "audio")["job_id"]
        .as_str()
        .expect("audio job");
    let job = get_ok(&app, &token, &format!("/api/pipeline/jobs/{job_id}")).await;
    assert!(
        job["error_message"]
            .as_str()
            .is_some_and(|error| error.contains(&format!(
                "Output file already exists and overwrite is disabled: {audio}"
            ))),
        "{job}"
    );
    assert_eq!(read(&audio), extracted);
    assert!(exists(&source));

    app.shutdown().await;
}

/// ffmpeg's filter syntax gives quotes, colons, commas and brackets meaning,
/// so the path of a subtitle in a folder named with all of them has to reach
/// the subtitles filter escaped for it to be burned in. Colons cannot appear
/// in Windows file names.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires ffmpeg with libx264 and libass, and ffprobe"]
async fn ass_burnin_reads_subtitles_from_a_path_with_quotes_colons_and_brackets() {
    const ASS: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 32\nPlayResY: 32\n\n\
        [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, Alignment\n\
        Style: Default,Sans,8,&H00FFFFFF,2\n\n\
        [Events]\nFormat: Layer, Start, End, Style, Text\n\
        Dialogue: 0,0:00:00.00,0:00:04.00,Default,hi\n";
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let folder = "rec/Let's [play]: part 1, take 2";
    let video = recording(&app, &format!("{folder}/show.flv"));
    let subtitles = app.write_output_file(&format!("{folder}/show.ass"), ASS);

    let dag = run_workflow(
        &app,
        &token,
        &[&video, &subtitles],
        vec![inline("burn", "ass_burnin", json!({}), &[])],
    )
    .await;
    assert_eq!(dag["status"], "COMPLETED", "{dag}");
    let burned = app.output_path(&format!("{folder}/show_burnin.flv"));
    // The inputs are passed on ahead of the result by default.
    assert_eq!(
        step(&dag, "burn")["outputs"],
        json!([video, subtitles, burned])
    );
    let probed = probe(&burned).await;
    let mut found = codecs(&probed);
    found.sort_unstable();
    assert_eq!(found, ["aac", "h264"], "{probed}");
    assert!(exists(&video) && exists(&subtitles));

    app.shutdown().await;
}

/// With automatic thumbnails on, the first segment of a recording gets a
/// JPEG thumbnail at the stream's own size, made by ffmpeg, which becomes
/// the session's cover. The recording is shorter than the preset's seek, so
/// the first frame is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires ffmpeg and ffprobe"]
async fn automatic_thumbnails_give_a_recorded_session_a_jpeg_cover() {
    let fake = FakePlatform::start().await;
    let app = TestApp::start_with(Seed::live().global("auto_thumbnail", 1)).await;
    let token = app.admin_token().await;
    let (_, session_id) = record(&app, &token, &fake, "Covered", Value::Null).await;

    let dags = finished_dags(&app, &token, &session_id).await;
    assert_eq!(dags.len(), 1, "{dags:?}");
    assert_eq!(dags[0]["status"], "COMPLETED", "{dags:?}");
    let outputs = get_ok(
        &app,
        &token,
        &format!("/api/pipeline/outputs?session_id={session_id}&file_type=THUMBNAIL"),
    )
    .await;
    let thumbnails = outputs["items"].as_array().expect("output items");
    assert_eq!(thumbnails.len(), 1, "{outputs}");
    let path = thumbnails[0]["file_path"].as_str().expect("thumbnail path");
    assert!(path.ends_with(".jpg"), "{path}");
    let probed = probe(path).await;
    assert_eq!(codecs(&probed), ["mjpeg"], "{probed}");
    assert_eq!(probed["streams"][0]["width"], 32, "{probed}");

    let session = get_ok(&app, &token, &format!("/api/sessions/{session_id}")).await;
    let cover = session["thumbnail_url"]
        .as_str()
        .unwrap_or_else(|| panic!("no thumbnail_url: {session}"));
    let response = app
        .client()
        .get(app.url(&format!("{cover}?token={token}")))
        .send()
        .await
        .expect("fetch the cover");
    assert_eq!(response.status(), StatusCode::OK);
    let served = response.bytes().await.expect("cover bytes");
    assert_eq!(served.as_ref(), read(path).as_slice());

    app.shutdown().await;
}

/// An rclone step in the session-complete workflow copies the recording to a
/// local destination named after the streamer, and the session's output
/// lists the completed upload with where it went.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires rclone"]
async fn an_rclone_step_copies_a_session_to_a_local_destination_and_records_the_upload() {
    let fake = FakePlatform::start().await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let remote = app.output_path("remote");
    let workflow = json!({
        "name": "upload",
        "steps": [inline(
            "upload",
            "rclone",
            json!({ "destination_root": format!("{remote}/{{streamer}}") }),
            &[],
        )],
    });
    let (streamer, session_id) = record(
        &app,
        &token,
        &fake,
        "Uploader",
        json!({ "session_complete_pipeline": workflow }),
    )
    .await;

    let dags = finished_dags(&app, &token, &session_id).await;
    assert_eq!(dags.len(), 1, "{dags:?}");
    assert_eq!(dags[0]["status"], "COMPLETED", "{dags:?}");
    let outputs = get_ok(
        &app,
        &token,
        &format!("/api/pipeline/outputs?session_id={session_id}"),
    )
    .await;
    let items = outputs["items"].as_array().expect("output items");
    assert_eq!(items.len(), 1, "{outputs}");
    let local = items[0]["file_path"].as_str().expect("recording path");
    let name = Path::new(local)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("recording file name");
    let copied = format!("{remote}/Uploader/{name}");
    assert_eq!(read(&copied), read(local));
    let uploads = items[0]["uploads"].as_array().expect("uploads");
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert_eq!(uploads[0]["uploader"], "rclone");
    assert_eq!(uploads[0]["status"], "COMPLETED", "{uploads:?}");
    assert_eq!(uploads[0]["remote_path"], copied.as_str());
    assert_eq!(items[0]["streamer_id"], streamer.as_str());

    app.shutdown().await;
}
