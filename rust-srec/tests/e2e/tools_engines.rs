//! Recording through the external download engines, ffmpeg and Streamlink,
//! and through the Streamlink pseudo-platform that takes URLs no built-in
//! platform claims.
//!
//! These need ffmpeg, ffprobe and Streamlink on `PATH`, so they are ignored
//! in the plain suite; CI's tool-gated job runs them.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::tools::{HlsOrigin, codecs, duration_secs, probe};
use crate::support::{FakePlatform, FlvLoop, Seed, TestApp, WAIT, eventually, id_of};

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// Wait for the streamer's only session to end and return its detail.
async fn ended_session(app: &TestApp, token: &str, streamer_id: &str) -> Value {
    let path = format!("/api/sessions?streamer_id={streamer_id}&include_empty=true");
    let id = eventually(WAIT, "the session to end", || async {
        let page = get_ok(app, token, &path).await;
        let sessions = page["items"].as_array().expect("session items");
        assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
        sessions
            .first()
            .filter(|session| session["is_live"] == false)
            .map(|session| session["id"].as_str().expect("session id").to_owned())
    })
    .await;
    get_ok(app, token, &format!("/api/sessions/{id}")).await
}

/// The session's stored segments, oldest first.
async fn segments(app: &TestApp, token: &str, session_id: &str) -> Vec<Value> {
    let page = get_ok(app, token, &format!("/api/sessions/{session_id}/segments")).await;
    let mut items = page["items"].as_array().expect("segment items").clone();
    items.sort_by_key(|segment| segment["segment_index"].as_i64());
    items
}

/// The path of `segment`, checked against the size the backend stored for
/// it.
#[track_caller]
fn stored_path(segment: &Value) -> String {
    let path = segment["file_path"].as_str().expect("segment file path");
    let size = std::fs::metadata(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .len();
    assert_eq!(segment["size_bytes"], size, "{segment}");
    path.to_owned()
}

/// The path of the session's only segment.
async fn only_segment(app: &TestApp, token: &str, session: &Value) -> String {
    let segments = segments(app, token, session["id"].as_str().expect("session id")).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    stored_path(&segments[0])
}

/// Panics unless `path` is an FLV file of H.264 video and AAC audio lasting
/// `secs` seconds, give or take a second (the FLV fixture has one video
/// frame a second).
async fn assert_flv_recording(path: &str, secs: f64) {
    assert!(path.ends_with(".flv"), "{path}");
    let header = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(&header[..3], b"FLV", "{path}");
    let probed = probe(path).await;
    let mut found = codecs(&probed);
    found.sort_unstable();
    assert_eq!(found, ["aac", "h264"], "{probed}");
    let duration = duration_secs(&probed);
    assert!((duration - secs).abs() <= 1.0, "{duration} s: {probed}");
}

/// The ffmpeg engine fetches the room's FLV itself, with the headers the
/// platform answered with, and writes the stream as one FLV file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires ffmpeg and ffprobe"]
async fn the_ffmpeg_engine_records_a_live_flv_room_with_the_platform_headers() {
    let fake = FakePlatform::start().await;
    let room = fake.room("ffmpeg");
    room.set_live(
        room.live("Through ffmpeg")
            .header("Referer", "https://fake.invalid/ffmpeg"),
    );
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = id_of(
        &app.create_enabled_streamer(
            &token,
            json!({
                "name": "Ffmpeg",
                "url": room.url(),
                "streamer_specific_config": { "download_engine": "default-ffmpeg" },
            }),
        )
        .await,
    );

    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(3));
    room.set_offline();
    room.end();

    let session = ended_session(&app, &token, &id).await;
    let path = only_segment(&app, &token, &session).await;
    assert_flv_recording(&path, 6.0).await;
    assert_eq!(
        session["total_size_bytes"],
        std::fs::metadata(&path).unwrap().len()
    );

    let media_requests = fake.requests_to(&room, "live.flv");
    assert_eq!(media_requests.len(), 1, "{media_requests:?}");
    let request = &media_requests[0];
    assert_eq!(
        request.header("referer"),
        Some("https://fake.invalid/ffmpeg")
    );
    // ffmpeg's own HTTP client made the request.
    assert!(
        request
            .header("user-agent")
            .is_some_and(|agent| agent.starts_with("Lavf/")),
        "{request:?}"
    );
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}

/// The Streamlink engine follows the room's live HLS playlist with the
/// platform's headers until the playlist ends, and ffmpeg remuxes what it
/// pipes into the configured FLV container.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Streamlink, ffmpeg and ffprobe"]
async fn the_streamlink_engine_records_a_live_hls_room_with_the_platform_headers() {
    let origin = HlsOrigin::start(3).await;
    let fake = FakePlatform::start().await;
    let room = fake.room("streamlink");
    room.set_live(
        room.live("Through Streamlink")
            .header("Referer", "https://fake.invalid/streamlink")
            .stream(json!({
                "url": origin.playlist_url(),
                "format": "hls",
                "media": "ts",
                "quality": "origin",
                "bitrate": 16_000,
            })),
    );
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = id_of(
        &app.create_enabled_streamer(
            &token,
            json!({
                "name": "Streamlink",
                "url": room.url(),
                "streamer_specific_config": {
                    "download_engine": "default-streamlink",
                    "stream_selection_config": { "preferred_formats": ["hls"] },
                },
            }),
        )
        .await,
    );

    origin.wait_for_segment(2).await;
    room.set_offline();
    origin.end();

    let session = ended_session(&app, &token, &id).await;
    let path = only_segment(&app, &token, &session).await;
    assert_flv_recording(&path, 3.0).await;
    for index in 0..3 {
        let requests = origin.segment_requests(index);
        assert_eq!(requests.len(), 1, "segment {index}: {requests:?}");
        assert_eq!(
            requests[0].header("referer"),
            Some("https://fake.invalid/streamlink"),
            "segment {index}"
        );
    }
    assert_eq!(fake.requests_to(&room, "live.flv").len(), 0);
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}

/// A URL no built-in platform claims but Streamlink can handle belongs to
/// the Streamlink pseudo-platform: Streamlink reports it live, and the
/// platform's default Streamlink engine records it.
///
/// Streamlink reports any reachable playlist live, ended or not, so the
/// backend resumes the session after a download completes; the test ends
/// it by disabling the streamer once the first part is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Streamlink, ffmpeg and ffprobe"]
async fn a_url_only_streamlink_handles_is_monitored_and_recorded_through_streamlink() {
    let origin = HlsOrigin::start(3).await;
    // Streamlink runs as its own process and takes the pseudo-platform's
    // route, so that route reaches the loopback origin; every other platform
    // stays on the dead end.
    let app = TestApp::start_with(Seed::live().rows(
        "UPDATE platform_config SET proxy_route = 'direct' WHERE id = 'platform-streamlink';\n",
    ))
    .await;
    let token = app.admin_token().await;
    let created = app
        .create_enabled_streamer(
            &token,
            json!({ "name": "Bare playlist", "url": origin.playlist_url() }),
        )
        .await;
    assert_eq!(created["platform_config_id"], "platform-streamlink");
    let id = id_of(&created);

    origin.wait_for_segment(2).await;
    origin.end();
    let sessions = format!("/api/sessions?streamer_id={id}&include_empty=true");
    let first = eventually(WAIT, "the first part to be stored", || async {
        let page = get_ok(&app, &token, &sessions).await;
        let session_id = page["items"][0]["id"].as_str()?.to_owned();
        segments(&app, &token, &session_id).await.into_iter().next()
    })
    .await;
    app.put(
        &format!("/api/streamers/{id}"),
        &token,
        &json!({ "enabled": false }),
    )
    .await
    .expect_status(StatusCode::OK);

    ended_session(&app, &token, &id).await;
    assert_eq!(first["segment_index"], 0, "{first}");
    assert_flv_recording(&stored_path(&first), 3.0).await;

    // The history lists the oldest check first.
    let history = get_ok(&app, &token, &format!("/api/streamers/{id}/check-history")).await;
    let check = &history["items"][0];
    assert_eq!(check["outcome"], "live", "{history}");
    // Streamlink names a media playlist's one stream `live` and offers it as
    // `best` and `worst` too; the best is recorded.
    assert_eq!(check["streams_extracted"], 3, "{history}");
    assert_eq!(check["stream_selected"]["quality"], "best", "{history}");
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}
