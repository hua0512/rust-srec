//! Recording through the external download engines, ffmpeg and Streamlink,
//! and through the Streamlink pseudo-platform that takes URLs no built-in
//! platform claims.
//!
//! These need ffmpeg, ffprobe and Streamlink on `PATH`, so they are ignored
//! in the plain suite; CI's tool-gated job runs them.

use serde_json::{Value, json};

use crate::support::tools::{DashOrigin, HlsOrigin, codecs, duration_secs, probe};
use crate::support::{FakePlatform, FlvLoop, Seed, TestApp, WAIT, eventually, id_of, millis};

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
    let segments = app
        .segments(token, session["id"].as_str().expect("session id"))
        .await;
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

    let session = app.ended_session(&token, &id).await;
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

    let session = app.ended_session(&token, &id).await;
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

/// Create an enabled streamer for `url`, which no built-in platform claims,
/// and return its id once it belongs to the Streamlink pseudo-platform.
async fn monitor_through_streamlink(url: &str) -> (TestApp, String, String) {
    // Streamlink runs as its own process and takes the pseudo-platform's
    // route, so that route reaches the loopback origin; every other platform
    // stays on the dead end.
    let app = TestApp::start_with(Seed::live().rows(
        "UPDATE platform_config SET proxy_route = 'direct' WHERE id = 'platform-streamlink';\n",
    ))
    .await;
    let token = app.admin_token().await;
    let created = app
        .create_enabled_streamer(&token, json!({ "name": "Bare URL", "url": url }))
        .await;
    assert_eq!(created["platform_config_id"], "platform-streamlink");
    let id = id_of(&created);
    (app, token, id)
}

/// Panics unless the streamer was recorded once, by Streamlink's best
/// stream into one three-second FLV file, and every check after that
/// session ended found it offline.
async fn assert_recorded_once(app: &TestApp, token: &str, id: &str) {
    let session = app.ended_session(token, id).await;
    let segments = app.segments(token, id_of(&session).as_str()).await;
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_flv_recording(&stored_path(&segments[0]), 3.0).await;

    // History rows are written in check order and after the check, so one
    // from after the end means every earlier row is in. The history lists
    // the oldest check first.
    let ended_at = millis(&session["end_time"]);
    let history = eventually(WAIT, "a check after the end", || async {
        let history = app.checks(token, id).await;
        history
            .iter()
            .any(|check| millis(&check["checked_at"]) > ended_at)
            .then_some(history)
    })
    .await;
    let check = &history[0];
    assert_eq!(check["outcome"], "live", "{history:?}");
    // Streamlink names a media playlist's one stream `live` and a DASH
    // presentation's by its resolution, and offers it as `best` and `worst`
    // too; the best is recorded.
    assert_eq!(check["streams_extracted"], 3, "{history:?}");
    assert_eq!(check["stream_selected"]["quality"], "best", "{history:?}");
    assert!(
        history
            .iter()
            .filter(|check| millis(&check["checked_at"]) > ended_at)
            .all(|check| check["outcome"] == "offline"),
        "{history:?}"
    );
    assert_eq!(app.sessions(token, id).await.len(), 1);
    assert_eq!(app.streamer(token, id).await["state"], "NOT_LIVE");
    assert_eq!(app.dead_end().attempts(), []);
}

/// A URL no built-in platform claims but Streamlink can handle belongs to
/// the Streamlink pseudo-platform: Streamlink reports it live, and the
/// platform's default Streamlink engine records it. Once the playlist has
/// ended it reads offline though it is still served, so it is recorded once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Streamlink, ffmpeg and ffprobe"]
async fn a_url_only_streamlink_handles_is_monitored_and_recorded_through_streamlink() {
    let origin = HlsOrigin::start(3).await;
    let (app, token, id) = monitor_through_streamlink(&origin.playlist_url()).await;

    origin.wait_for_segment(2).await;
    origin.end();
    assert_recorded_once(&app, &token, &id).await;

    app.shutdown().await;
}

/// A bare DASH manifest is recorded through Streamlink the same way, and
/// reads offline once its MPD turns static.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Streamlink, ffmpeg and ffprobe"]
async fn a_url_only_streamlink_dash_manifest_is_recorded_once_it_ends() {
    let origin = DashOrigin::start(3).await;
    let (app, token, id) = monitor_through_streamlink(&origin.manifest_url()).await;

    origin.wait_for_last_segments().await;
    origin.end();
    assert_recorded_once(&app, &token, &id).await;

    app.shutdown().await;
}
