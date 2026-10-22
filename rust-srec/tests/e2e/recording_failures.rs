//! Recordings that fail to start, are rate limited, or are cancelled before
//! they start, on fake-platform rooms whose streams a [`FakeCdn`] serves.
//!
//! Apps here are seeded with [`Seed::live`]: one offline check confirms an
//! end, and two failures in a row put the streamer into its 60 s error
//! backoff. A failed start parks its session for two offline-check intervals,
//! widened from 1 s to 2 s ([`parking`]) so a loaded machine still retries
//! the start inside them.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::cdn::{FakeCdn, HlsMedia};
use crate::support::{
    FakePlatform, FakeRoom, Seed, TestApp, WAIT, eventually, id_of, millis, now_ms,
};

/// [`Seed::live`] with offline checks 2 s apart.
fn parking() -> Seed {
    Seed::live().global("offline_check_delay_ms", 2_000)
}

/// A stream at `url` that outranks the FLV stream every live answer of the
/// room also offers.
fn cdn_stream(url: &str, format: &str, media: &str) -> Value {
    json!({ "url": url, "format": format, "media": media, "quality": "origin", "bitrate": 1_000_000 })
}

async fn watch(app: &TestApp, token: &str, name: &str, room: &FakeRoom) -> String {
    id_of(
        &app.create_enabled_streamer(token, json!({ "name": name, "url": room.url() }))
            .await,
    )
}

fn events(session: &Value) -> Vec<&Value> {
    session["events"]
        .as_array()
        .expect("session events")
        .iter()
        .map(|event| &event["payload"])
        .collect()
}

fn started(from_hysteresis: bool, title: &str) -> Value {
    json!({ "kind": "session_started", "from_hysteresis": from_hysteresis, "title": title })
}

fn resumed() -> Value {
    json!({ "kind": "session_resumed", "hysteresis_duration_secs": 0 })
}

/// Panics unless `event` parks the session after a start that failed with
/// `kind`. Its deadline is not compared.
#[track_caller]
fn failed_start(event: &Value, kind: &str) {
    assert_eq!(event["kind"], "hysteresis_entered", "{event}");
    assert_eq!(
        event["cause"],
        json!({ "type": "failed", "kind": kind }),
        "{event}"
    );
}

async fn segment_count(app: &TestApp, token: &str, session: &Value) -> usize {
    let id = session["id"].as_str().expect("session id");
    app.segments(token, id).await.len()
}

/// A start the server refuses, here with 403 for an FLV stream and with DRM
/// in a DASH presentation, is tried once more at once. When that fails too,
/// the session ends as failed rather than staying open, and the streamer
/// shows the error and backs off instead of trying again. Neither
/// recording fetches any media.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_start_ends_its_session_and_backs_the_streamer_off() {
    let fake = FakePlatform::start().await;
    let cdn = FakeCdn::start().await;
    cdn.put("/forbidden/live.flv", "video/x-flv", "FLV");
    cdn.fail("/forbidden/live.flv", 10, 403, &[]);
    let protected = cdn.publish_protected_dash("protected");
    let cases = [
        (
            "forbidden",
            cdn_stream(&cdn.url("/forbidden/live.flv"), "flv", "flv"),
            "/forbidden/live.flv",
            "http_client_error",
            "HTTP 403 Forbidden",
        ),
        (
            "protected",
            cdn_stream(&cdn.url(&protected), "dash", "fmp4"),
            protected.as_str(),
            "configuration",
            "content protection (DRM)",
        ),
    ];
    let app = TestApp::start_with(parking()).await;
    let token = app.admin_token().await;

    for (name, stream, media_path, kind, error) in cases {
        let room = fake.room(name);
        room.set_live(room.live(name).stream(stream));
        let id = watch(&app, &token, name, &room).await;

        let session = app.ended_session(&token, &id).await;
        let events = events(&session);
        assert_eq!(events.len(), 6, "{name}: {events:?}");
        assert_eq!(events[0], &started(false, name), "{name}");
        failed_start(events[1], kind);
        assert_eq!(events[2], &resumed(), "{name}");
        assert_eq!(events[3], &started(true, name), "{name}");
        failed_start(events[4], kind);
        assert_eq!(
            events[5],
            &json!({
                "kind": "session_ended",
                "cause": { "type": "failed", "kind": kind },
                "via_hysteresis": true,
            }),
            "{name}"
        );
        assert_eq!(segment_count(&app, &token, &session).await, 0, "{name}");

        let backing_off = eventually(WAIT, "the streamer to back off", || async {
            let streamer = app.streamer(&token, &id).await;
            (streamer["state"] == "TEMPORAL_DISABLED").then_some(streamer)
        })
        .await;
        assert_eq!(backing_off["consecutive_error_count"], 2, "{backing_off}");
        let last_error = backing_off["last_error"].as_str().unwrap_or_default();
        assert!(last_error.contains(error), "{name}: {last_error}");
        let backoff_ms = millis(&backing_off["disabled_until"]) - now_ms();
        assert!(
            (30_000..=60_000).contains(&backoff_ms),
            "{name}: backing off for {backoff_ms} ms"
        );
        // One request per start: the backoff, set before the session ended,
        // has stopped the retries.
        assert_eq!(cdn.requests_to(media_path).len(), 2, "{name}");
        assert_eq!(fake.requests_to(&room, "live.flv").len(), 0, "{name}");
    }
    // The DRM presentation's media was never asked for.
    let protected_requests: Vec<String> = cdn
        .requests()
        .into_iter()
        .map(|request| request.path)
        .filter(|path| path.starts_with("/protected/"))
        .collect();
    assert_eq!(protected_requests, [protected.as_str(); 2]);
    assert_eq!(cdn.queued_failures("/forbidden/live.flv"), 8);

    app.shutdown().await;
}

/// A rate-limited playlist is asked for again, both when it refuses the
/// start of a recording and while the recording refreshes it, and the
/// recording completes in the same session with no error left on the
/// streamer. Refused at the start, the recording fails and resumes at once;
/// refused on a refresh, the recording waits out the `Retry-After` without
/// failing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_playlists_are_retried_within_the_session() {
    let fake = FakePlatform::start().await;
    let cdn = FakeCdn::start().await;
    let app = TestApp::start_with(parking()).await;
    let token = app.admin_token().await;
    let retry_after = [("retry-after", "1")];

    for (name, limited_at_start) in [("at-start", true), ("on-refresh", false)] {
        let room = fake.room(name);
        let playlist = cdn.publish_hls(name, HlsMedia::Ts, 1, false);
        if limited_at_start {
            cdn.fail(&playlist, 1, 429, &retry_after);
        }
        room.set_live(
            room.live(name)
                .stream(cdn_stream(&cdn.url(&playlist), "hls", "ts")),
        );
        let id = watch(&app, &token, name, &room).await;

        cdn.wait_for_requests(&format!("/{name}/segment0.ts"), 1)
            .await;
        let before_refusal = cdn.requests_to(&playlist).len();
        if !limited_at_start {
            cdn.fail(&playlist, 1, 429, &retry_after);
        }
        room.set_offline();
        cdn.publish_hls(name, HlsMedia::Ts, 2, true);

        let session = app.ended_session(&token, &id).await;
        let events = events(&session);
        let completed = json!({
            "kind": "session_ended", "cause": { "type": "completed" }, "via_hysteresis": false,
        });
        if limited_at_start {
            assert_eq!(events.len(), 5, "{events:?}");
            assert_eq!(events[0], &started(false, name));
            failed_start(events[1], "rate_limited");
            assert_eq!(events[2], &resumed());
            assert_eq!(events[3], &started(true, name));
            assert_eq!(events[4], &completed);
        } else {
            assert_eq!(events, [&started(false, name), &completed]);
        }
        assert_eq!(segment_count(&app, &token, &session).await, 1, "{name}");

        // The refusal was served, and the playlist asked for again after it.
        assert_eq!(cdn.queued_failures(&playlist), 0, "{name}");
        if !limited_at_start {
            assert!(cdn.requests_to(&playlist).len() >= before_refusal + 2);
        }
        let recovered = eventually(WAIT, "the error state to clear", || async {
            let streamer = app.streamer(&token, &id).await;
            (streamer["consecutive_error_count"] == 0).then_some(streamer)
        })
        .await;
        assert!(recovered["last_error"].is_null(), "{recovered}");
        assert!(recovered["disabled_until"].is_null(), "{recovered}");
    }

    app.shutdown().await;
}

/// A recording waiting for a download slot is cancelled when its streamer
/// is disabled: it never starts, its session ends as a user stop, and no
/// workflow runs for it although the platform has a session-complete one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recording_cancelled_while_queued_never_starts_and_runs_no_workflow() {
    let fake = FakePlatform::start().await;
    let running = fake.room("running");
    running.set_live(running.live("Holding the slot"));
    let waiting = fake.room("waiting");
    waiting.set_live(waiting.live("Waiting"));
    let complete = json!({
        "name": "session-complete",
        "steps": [{
            "id": "succeed",
            "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
            "depends_on": [],
        }],
    });
    let app = TestApp::start_with(
        Seed::live()
            .global("max_concurrent_downloads", 1)
            .session_complete_pipeline(&complete),
    )
    .await;
    let token = app.admin_token().await;
    watch(&app, &token, "Running", &running).await;
    running.wait_for_media_connection(1).await;
    let id = watch(&app, &token, "Waiting", &waiting).await;
    let pending = || async {
        let idle = app
            .send(reqwest::Method::GET, "/api/health/idle", None, None)
            .await;
        idle.body["pending_recordings"].as_u64()
    };
    eventually(WAIT, "the recording to wait for a slot", || async {
        (pending().await == Some(1)).then_some(())
    })
    .await;

    app.put(
        &format!("/api/streamers/{id}"),
        &token,
        &json!({ "enabled": false }),
    )
    .await
    .expect_status(StatusCode::OK);

    let session = app.ended_session(&token, &id).await;
    assert_eq!(
        events(&session),
        [
            &started(false, "Waiting"),
            &json!({ "kind": "session_ended", "cause": { "type": "user_disabled" }, "via_hysteresis": false }),
        ]
    );
    let session_id = session["id"].as_str().expect("session id");
    // Once the session-complete decision is made, no workflow is added.
    let decided =
        format!("SELECT session_complete_dispatched FROM live_sessions WHERE id = '{session_id}'");
    eventually(WAIT, "the session-complete decision", || async {
        (app.query_i64(&decided).await == 1).then_some(())
    })
    .await;
    let dags = app
        .get_ok(
            &format!("/api/pipeline/dags?session_id={session_id}"),
            &token,
        )
        .await;
    assert_eq!(dags["dags"], json!([]), "{dags}");
    assert_eq!(segment_count(&app, &token, &session).await, 0);
    assert_eq!(pending().await, Some(0));
    assert_eq!(fake.requests_to(&waiting, "live.flv").len(), 0);
    // The recording holding the slot carries on.
    assert!(!running.media_closed());

    app.shutdown().await;
}
