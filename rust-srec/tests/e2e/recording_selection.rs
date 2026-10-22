//! Which of a room's streams a recording uses, and what its media requests
//! carry. How the selection fields rank and merge is pinned by the unit tests
//! of `downloader::stream_selector`; these pin that each preference a
//! streamer sets reaches the recording.

use serde_json::{Value, json};

use crate::support::cdn::{FakeCdn, HlsMedia};
use crate::support::{FakePlatform, FakeRoom, ReceivedRequest, Seed, TestApp, id_of};

/// A stream of `room` at its FLV endpoint, told apart by `pick` in its
/// query.
fn candidate(room: &FakeRoom, pick: &str, format: &str, quality: &str, bitrate: u64) -> Value {
    json!({
        "url": format!("{}?pick={pick}", room.flv_url()),
        "format": format,
        "media": "flv",
        "quality": quality,
        "bitrate": bitrate,
    })
}

/// Preferred qualities rank before bitrate, a preferred format before
/// bitrate, and a maximum bitrate excludes the streams above it. Every room
/// offers the same streams; the room's own stream (`origin`, 16 kbps, FLV)
/// is what a streamer without preferences records.
///
/// The HLS-labelled candidate is served at an FLV URL, which the recording
/// engine records as FLV; only its label takes part in the selection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quality_format_and_bitrate_preferences_pick_the_recorded_stream() {
    let fake = FakePlatform::start().await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let cases = [
        ("default", json!({}), None),
        (
            "quality",
            json!({ "preferred_qualities": ["720p"] }),
            Some("720p"),
        ),
        (
            "format",
            json!({ "preferred_formats": ["hls"] }),
            Some("hls"),
        ),
        ("bitrate", json!({ "max_bitrate": 10_000 }), Some("8k")),
    ];
    let mut rooms = Vec::new();
    for (name, selection, expected) in cases {
        let room = fake.room(name);
        room.set_live(
            room.live(name)
                .stream(candidate(&room, "720p", "flv", "720p", 4_000))
                .stream(candidate(&room, "hls", "hls", "origin", 2_000))
                .stream(candidate(&room, "8k", "flv", "480p", 8_000))
                .stream(candidate(&room, "12k", "flv", "540p", 12_000)),
        );
        app.create_enabled_streamer(
            &token,
            json!({
                "name": name,
                "url": room.url(),
                "streamer_specific_config": { "stream_selection_config": selection },
            }),
        )
        .await;
        rooms.push((room, expected));
    }

    for (room, expected) in &rooms {
        room.wait_for_media_connection(1).await;
        let media = fake.requests_to(room, "live.flv");
        assert_eq!(media.len(), 1, "{media:?}");
        let query = media[0].path.split_once('?').map(|(_, query)| query);
        assert_eq!(
            query,
            expected.map(|pick| format!("pick={pick}")).as_deref(),
            "{}",
            room.url()
        );
    }

    app.shutdown().await;
}

/// The headers of an HLS recording's playlist and segment requests come from
/// three places: the platform's answer for the room, the chosen stream's own
/// headers and `Host`, which replace the room's by name in any case, and the
/// user agent a template sets on the recording engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_stream_and_engine_headers_reach_every_hls_request() {
    let fake = FakePlatform::start().await;
    let cdn = FakeCdn::start().await;
    let room = fake.room("headers");
    let playlist = cdn.publish_hls("headers", HlsMedia::Ts, 2, true);
    room.set_live(
        room.live("Headers")
            .header("Referer", "https://room.invalid/")
            .header("X-Room", "room")
            .stream(json!({
                "url": cdn.url(&playlist),
                "format": "hls",
                "media": "ts",
                "quality": "origin",
                "bitrate": 1_000_000,
                "extras": {
                    "headers": { "referer": "https://stream.invalid/", "X-Edge": "edge-1" },
                    "host_header": "cdn.invalid",
                },
            })),
    );
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Own agent",
                "engines_override": {
                    "default-mesio": { "hls": { "base": { "user_agent": "E2E-Agent/1.0" } } },
                },
            }),
        )
        .await
        .created();
    app.create_enabled_streamer(
        &token,
        json!({ "name": "Headers", "url": room.url(), "template_id": id_of(&template) }),
    )
    .await;

    let segments = ["/headers/segment0.ts", "/headers/segment1.ts"];
    for segment in segments {
        cdn.wait_for_requests(segment, 1).await;
    }
    let requests: Vec<ReceivedRequest> = std::iter::once(playlist.as_str())
        .chain(segments)
        .flat_map(|path| cdn.requests_to(path))
        .collect();
    for request in &requests {
        let headers =
            ["referer", "x-room", "x-edge", "host", "user-agent"].map(|name| request.header(name));
        assert_eq!(
            headers,
            [
                Some("https://stream.invalid/"),
                Some("room"),
                Some("edge-1"),
                Some("cdn.invalid"),
                Some("E2E-Agent/1.0"),
            ],
            "{request:?}"
        );
        let referers = request
            .headers
            .iter()
            .filter(|(name, _)| name == "referer")
            .count();
        assert_eq!(referers, 1, "{request:?}");
    }

    app.shutdown().await;
}
