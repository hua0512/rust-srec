//! Monitoring: what the scheduler does with each answer a platform gives.
//!
//! Every app here is seeded with [`Seed::live`]: checks every 200 ms, at most
//! about one per second per platform after a burst of two, and a global route
//! that ends at a recorder answering 502.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{FakePlatform, Seed, TestApp, WAIT, eventually, id_of, millis};

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

async fn watch(app: &TestApp, token: &str, name: &str, url: &str) -> String {
    id_of(
        &app.create_enabled_streamer(token, json!({ "name": name, "url": url }))
            .await,
    )
}

async fn streamer(app: &TestApp, token: &str, id: &str) -> Value {
    get_ok(app, token, &format!("/api/streamers/{id}")).await
}

/// The streamer's check history, oldest first.
async fn checks(app: &TestApp, token: &str, id: &str) -> Vec<Value> {
    get_ok(app, token, &format!("/api/streamers/{id}/check-history")).await["items"]
        .as_array()
        .expect("check history")
        .clone()
}

/// Wait until the streamer has `count` more checks recorded than it has now,
/// as proof that the scheduler kept running meanwhile.
async fn wait_for_more_checks(app: &TestApp, token: &str, id: &str, count: usize) {
    let target = checks(app, token, id).await.len() + count;
    eventually(WAIT, "more checks of the control streamer", || async {
        (checks(app, token, id).await.len() >= target).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_platform_answers_stop_monitoring_and_are_recorded_in_check_history() {
    let fake = FakePlatform::start().await;
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let cases = [
        (
            "not_found",
            "NOT_FOUND",
            "NotFound",
            "Streamer not found on platform",
        ),
        (
            "banned",
            "FATAL_ERROR",
            "Banned",
            "Streamer is banned on platform",
        ),
        (
            "age_restricted",
            "FATAL_ERROR",
            "AgeRestricted",
            "Content is age-restricted",
        ),
        (
            "region_locked",
            "FATAL_ERROR",
            "RegionLocked",
            "Content is region-locked",
        ),
        ("private", "FATAL_ERROR", "Private", "Content is private"),
    ];
    let control = fake.room("control");
    let control_id = watch(&app, &token, "Control", control.url()).await;
    let mut watched = Vec::new();
    for (kind, ..) in cases {
        let room = fake.room(kind);
        room.set_error(kind, "the platform says no");
        watched.push((room.clone(), watch(&app, &token, kind, room.url()).await));
    }

    for ((kind, state, _, last_error), (_, id)) in cases.iter().zip(&watched) {
        let stopped = eventually(WAIT, &format!("{kind} to stop monitoring"), || async {
            let streamer = streamer(&app, &token, id).await;
            (streamer["state"] != "NOT_LIVE").then_some(streamer)
        })
        .await;
        assert_eq!(stopped["state"], *state, "{kind}: {stopped}");
        assert_eq!(stopped["last_error"], *last_error, "{kind}");
        assert_eq!(stopped["consecutive_error_count"], 0, "{kind}");
        assert!(stopped["disabled_until"].is_null(), "{kind}: {stopped}");
    }

    // The scheduler keeps checking the control streamer, but never again the
    // fatal ones. History rows are written in check order, so the control's
    // later rows mean the fatal checks' rows are in too.
    wait_for_more_checks(&app, &token, &control_id, 2).await;
    for ((kind, _, fatal_kind, _), (room, id)) in cases.iter().zip(&watched) {
        assert_eq!(fake.requests_to(room, "status").len(), 1, "{kind}");
        let history = checks(&app, &token, id).await;
        assert_eq!(history.len(), 1, "{kind}: {history:?}");
        assert_eq!(history[0]["outcome"], "fatal_error", "{kind}");
        assert_eq!(history[0]["fatal_kind"], *fatal_kind, "{kind}");
        assert!(history[0]["error_message"].is_null(), "{kind}");
    }

    app.shutdown().await;
}

/// Start an app watching a room whose checks fail, and return it with the
/// streamer's id once the streamer has backed off and the scheduler, still
/// checking a control streamer, has stopped checking it.
async fn backing_off(fake: &FakePlatform) -> (TestApp, String, String) {
    let flaky = fake.room("flaky");
    flaky.set_error("http", "upstream unavailable");
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let id = watch(&app, &token, "Flaky", flaky.url()).await;
    let control_id = watch(&app, &token, "Control", fake.room("control").url()).await;
    eventually(WAIT, "the backoff", || async {
        (!streamer(&app, &token, &id).await["disabled_until"].is_null()).then_some(())
    })
    .await;
    wait_for_more_checks(&app, &token, &control_id, 2).await;
    assert_eq!(fake.requests_to(&flaky, "status").len(), 2);
    assert_eq!(checks(&app, &token, &id).await.len(), 2);
    (app, token, id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_check_failures_back_off_and_clear_error_resets_the_backoff() {
    let fake = FakePlatform::start().await;
    let (app, token, id) = backing_off(&fake).await;

    // With offline_check_count 1 the threshold is its floor of two failures,
    // and the first backoff lasts 60 s.
    let backing_off = streamer(&app, &token, &id).await;
    assert_eq!(backing_off["state"], "TEMPORAL_DISABLED", "{backing_off}");
    assert_eq!(backing_off["consecutive_error_count"], 2);
    assert_eq!(backing_off["last_error"], "Extraction failed (http)");
    let history = checks(&app, &token, &id).await;
    for check in &history {
        assert_eq!(check["outcome"], "transient_error", "{check}");
        assert_eq!(check["error_message"], "Extraction failed (http)");
    }
    let backoff = millis(&backing_off["disabled_until"]) - millis(&history[1]["checked_at"]);
    assert!((60_000..65_000).contains(&backoff), "{backoff} ms");

    let cleared = app
        .post(
            &format!("/api/streamers/{id}/clear-error"),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    let stored = streamer(&app, &token, &id).await;
    for streamer in [&cleared, &stored] {
        assert_eq!(streamer["state"], "NOT_LIVE", "{streamer}");
        assert_eq!(streamer["consecutive_error_count"], 0);
        assert!(streamer["disabled_until"].is_null(), "{streamer}");
        assert!(streamer["last_error"].is_null(), "{streamer}");
    }

    app.shutdown().await;
}

/// Clearing the error checks the streamer again right away instead of at the
/// end of its 60 s backoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_error_resumes_monitoring_before_the_backoff_ends() {
    let fake = FakePlatform::start().await;
    let (app, token, id) = backing_off(&fake).await;
    fake.room("flaky").set_offline();

    app.post(
        &format!("/api/streamers/{id}/clear-error"),
        &token,
        &json!({}),
    )
    .await
    .expect_status(StatusCode::OK);
    let history = eventually(WAIT, "a check after clear-error", || async {
        let history = checks(&app, &token, &id).await;
        (history.len() > 2).then_some(history)
    })
    .await;
    assert_eq!(history[2]["outcome"], "offline", "{history:?}");
    assert_eq!(streamer(&app, &token, &id).await["state"], "NOT_LIVE");

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_streamers_on_every_built_in_platform_fail_through_a_dead_end_route() {
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    // A page per platform and the domains its extractor's first requests go
    // to.
    let platforms: [(&str, &[&str]); 14] = [
        ("https://www.huya.com/660000", &["huya.com"]),
        (
            "https://live.douyin.com/123456789",
            &["douyin.com", "bytedance.com"],
        ),
        ("https://www.douyu.com/9999", &["douyu.com"]),
        (
            "https://www.pandalive.co.kr/play/someone",
            &["pandalive.co.kr"],
        ),
        ("https://weibo.com/u/1234567890", &["weibo.com"]),
        ("https://www.twitch.tv/someone", &["twitch.tv"]),
        (
            "https://xhslink.com/m/844vKmW30jz",
            &["xhslink.com", "xiaohongshu.com"],
        ),
        ("https://live.bilibili.com/4063253", &["bilibili.com"]),
        ("https://picarto.tv/someone", &["picarto.tv"]),
        ("https://www.tiktok.com/@someone/live", &["tiktok.com"]),
        ("https://twitcasting.tv/someone", &["twitcasting.tv"]),
        ("https://live.acfun.cn/live/123456", &["acfun.cn"]),
        (
            "https://play.sooplive.co.kr/someone/123",
            &["sooplive.co.kr", "sooplive.com"],
        ),
        ("https://www.bigo.tv/123456", &["bigo.tv", "bigo.sg"]),
    ];
    let mut ids = Vec::new();
    for (url, _) in platforms {
        ids.push(watch(&app, &token, url, url).await);
    }

    for ((url, _), id) in platforms.iter().zip(&ids) {
        let failed = eventually(WAIT, &format!("{url} to back off"), || async {
            let streamer = streamer(&app, &token, id).await;
            (!streamer["disabled_until"].is_null()).then_some(streamer)
        })
        .await;
        assert_eq!(failed["state"], "TEMPORAL_DISABLED", "{url}: {failed}");
        assert_eq!(failed["consecutive_error_count"], 2, "{url}");
        assert_eq!(failed["last_error"], "Extraction failed (http)", "{url}");
    }

    // Every request asked the recorder for a tunnel, so none resolved a name
    // or left the machine, and every platform's requests took that route.
    let attempts = app.dead_end().attempts();
    let hosts: Vec<&str> = attempts
        .iter()
        .map(|attempt| {
            assert_eq!(attempt.method, "CONNECT", "{attempt:?}");
            attempt
                .target
                .strip_suffix(":443")
                .unwrap_or_else(|| panic!("not an HTTPS tunnel: {attempt:?}"))
        })
        .collect();
    for (url, domains) in platforms {
        assert!(
            hosts.iter().any(|host| domains
                .iter()
                .any(|domain| host == domain || host.ends_with(&format!(".{domain}")))),
            "no request for {url} reached the route: {hosts:?}"
        );
    }

    app.shutdown().await;
}
