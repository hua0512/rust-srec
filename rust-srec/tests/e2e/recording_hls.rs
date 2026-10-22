//! Recording segmented streams that fake-platform rooms point at a
//! [`FakeCdn`] for: HLS with TS or fMP4 segments, and DASH. Unlike an FLV
//! body ending, the end of an HLS playlist (`#EXT-X-ENDLIST`) or of a DASH
//! presentation says the broadcast is over, so the session ends at once,
//! without waiting for a status check.
//!
//! Every app here is seeded with [`Seed::live`]. A room's live answer always
//! offers its own FLV stream too, so the segmented stream outranks it by
//! bitrate.

use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::cdn::{FakeCdn, HlsMedia};
use crate::support::{FakePlatform, FakeRoom, Seed, TestApp, WAIT, eventually, id_of};

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// A stream at `url` that outranks the room's own FLV stream.
fn segmented_stream(url: &str, format: &str, media: &str) -> Value {
    json!({ "url": url, "format": format, "media": media, "quality": "origin", "bitrate": 1_000_000 })
}

async fn watch(app: &TestApp, token: &str, name: &str, room: &FakeRoom) -> String {
    id_of(
        &app.create_enabled_streamer(token, json!({ "name": name, "url": room.url() }))
            .await,
    )
}

/// The streamer's sessions, newest first.
async fn sessions(app: &TestApp, token: &str, streamer_id: &str) -> Vec<Value> {
    get_ok(
        app,
        token,
        &format!("/api/sessions?streamer_id={streamer_id}&include_empty=true"),
    )
    .await["items"]
        .as_array()
        .expect("session items")
        .clone()
}

/// Wait for the streamer's first session to end and return its detail.
async fn first_session_ended(app: &TestApp, token: &str, streamer_id: &str) -> Value {
    let id = eventually(WAIT, "the session to end", || async {
        sessions(app, token, streamer_id)
            .await
            .last()
            .filter(|session| session["is_live"] == false)
            .map(|session| session["id"].as_str().expect("session id").to_owned())
    })
    .await;
    get_ok(app, token, &format!("/api/sessions/{id}")).await
}

/// The end of a session the stream itself ended.
fn stream_end() -> Value {
    json!({ "kind": "session_ended", "cause": { "type": "completed" }, "via_hysteresis": false })
}

fn events(session: &Value) -> Vec<&Value> {
    session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .map(|event| &event["payload"])
        .collect()
}

/// The session's only segment, after checking that it is a file on disk of
/// its recorded size, with extension `extension`, holding the whole stream.
async fn only_segment(app: &TestApp, token: &str, session: &Value, extension: &str) -> Vec<u8> {
    let id = session["id"].as_str().expect("session id");
    let page = get_ok(app, token, &format!("/api/sessions/{id}/segments")).await;
    let segments = page["items"].as_array().expect("segment items");
    assert_eq!(segments.len(), 1, "{segments:?}");
    let segment = &segments[0];
    assert_eq!(segment["duration_secs"], 2.0, "{segment}");
    assert_eq!(segment["split_reason_code"], "end_of_stream", "{segment}");
    let path = segment["file_path"].as_str().expect("segment file path");
    assert_eq!(
        Path::new(path).extension(),
        Some(extension.as_ref()),
        "{path}"
    );
    assert!(Path::new(path).starts_with(app.output_dir()), "{path}");
    let recorded = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(segment["size_bytes"], recorded.len(), "{segment}");
    assert_eq!(session["output_count"], 1, "{session}");
    assert_eq!(session["total_size_bytes"], recorded.len(), "{session}");
    recorded
}

/// A live HLS playlist gains its second segment and `#EXT-X-ENDLIST`; the
/// recording takes both segments into one part and ends the session as
/// completed, with TS segments and with fMP4 ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hls_recordings_end_with_the_playlist_for_ts_and_fmp4_segments() {
    let fake = FakePlatform::start().await;
    let cdn = FakeCdn::start().await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    for (media, segment, extension) in [
        (HlsMedia::Ts, "segment0.ts", "ts"),
        (HlsMedia::Fmp4, "segment0.m4s", "m4s"),
    ] {
        let name = media.media_format();
        let room = fake.room(name);
        let playlist = cdn.publish_hls(name, media, 1, false);
        room.set_live(room.live("Segmented").stream(segmented_stream(
            &cdn.url(&playlist),
            "hls",
            name,
        )));
        let id = watch(&app, &token, name, &room).await;

        cdn.wait_for_requests(&format!("/{name}/{segment}"), 1)
            .await;
        // The room goes offline first, so no check after the playlist ends
        // opens a second session.
        room.set_offline();
        cdn.publish_hls(name, media, 2, true);

        let session = first_session_ended(&app, &token, &id).await;
        assert_eq!(sessions(&app, &token, &id).await.len(), 1, "{name}");
        assert_eq!(
            events(&session),
            [
                &json!({ "kind": "session_started", "from_hysteresis": false, "title": "Segmented" }),
                &stream_end(),
            ],
            "{name}"
        );
        let recorded = only_segment(&app, &token, &session, extension).await;
        match media {
            // Whole 188-byte transport packets, each starting with the sync byte.
            HlsMedia::Ts => {
                assert_eq!(recorded.len() % 188, 0, "{name}");
                assert!(recorded.chunks(188).all(|packet| packet[0] == 0x47));
            }
            // The initialization section comes first.
            HlsMedia::Fmp4 => assert_eq!(&recorded[4..8], b"ftyp"),
        }
        assert_eq!(cdn.requests_to(&format!("/{name}/{segment}")).len(), 1);
        assert_eq!(fake.requests_to(&room, "live.flv").len(), 0);
    }

    app.shutdown().await;
}

/// A static DASH presentation is recorded whole, its video and audio tracks
/// merged into one fMP4 part, and its end ends the session as completed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dash_recording_ends_with_the_presentation() {
    let fake = FakePlatform::start().await;
    let cdn = FakeCdn::start().await;
    let room = fake.room("dash");
    let mpd = cdn.publish_dash("dash");
    room.set_live(
        room.live("Presented")
            .stream(segmented_stream(&cdn.url(&mpd), "dash", "fmp4")),
    );
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Dash", &room).await;

    cdn.wait_for_requests(&mpd, 1).await;
    // A presentation this short may end before the room goes offline; then
    // a later check can open a second session after the first has ended, so
    // only the first is asserted on.
    room.set_offline();

    let session = first_session_ended(&app, &token, &id).await;
    assert_eq!(
        events(&session),
        [
            &json!({ "kind": "session_started", "from_hysteresis": false, "title": "Presented" }),
            &stream_end(),
        ]
    );
    let recorded = only_segment(&app, &token, &session, "m4s").await;
    assert_eq!(&recorded[4..8], b"ftyp");
    // Both tracks were fetched to their last segment.
    for path in [
        "/dash/v/init.mp4",
        "/dash/v/t12288.m4s",
        "/dash/a/init.mp4",
        "/dash/a/t96256.m4s",
    ] {
        assert!(!cdn.requests_to(path).is_empty(), "{path}");
    }
    assert_eq!(fake.requests_to(&room, "live.flv").len(), 0);

    app.shutdown().await;
}
