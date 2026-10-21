//! URL parsing, offline only: every request here is refused or answered
//! before an extractor contacts a platform. BaiduPCS tool routes too.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{FakePlatform, Response, Seed, TestApp, WAIT};

/// The caching and referrer headers every parse answer carries.
#[track_caller]
fn assert_private(response: &Response) {
    assert_eq!(
        response.header(reqwest::header::CACHE_CONTROL),
        Some("private, no-store")
    );
    assert_eq!(
        response.header(reqwest::header::REFERRER_POLICY),
        Some("no-referrer")
    );
}

/// Whether a URL no built-in extractor claims would fall back to an installed
/// Streamlink, which could then try to reach it. A binary that does not
/// answer in time counts as installed, so the parse that could reach it is
/// skipped. CI's plain test jobs have no Streamlink on `PATH`, so the
/// unsupported-URL answer is checked there; the tool-gated job, which
/// installs Streamlink, runs only ignored tests.
async fn streamlink_available() -> bool {
    let binary = std::env::var("STREAMLINK_PATH").unwrap_or_else(|_| "streamlink".to_string());
    let probe = tokio::process::Command::new(binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(WAIT, probe).await {
        Ok(status) => status.is_ok_and(|status| status.success()),
        Err(_) => true,
    }
}

#[tokio::test]
async fn parse_validates_its_input_and_answers_privately() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    // A built-in platform URL; every request with it below is refused before
    // an extractor is created.
    const BILIBILI: &str = "https://live.bilibili.com/9100";

    for body in [
        json!({"url": BILIBILI, "cookies": "SESSDATA=1", "credential_id": "any"}),
        json!({"url": BILIBILI, "credential_id": "  "}),
        json!({"url": BILIBILI, "credential_id": "missing-profile"}),
        json!({"url": "http://203.0.113.7/live", "credential_id": "missing-profile"}),
    ] {
        let refused = app.post("/api/parse", &token, &body).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }

    let empty = app
        .post("/api/parse", &token, &json!({"url": ""}))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        empty.body,
        json!({"success": false, "is_live": false, "media_info": null, "error": "URL cannot be empty"})
    );
    assert_private(&empty);

    if streamlink_available().await {
        eprintln!("Streamlink is installed; skipping the unsupported-URL parse");
    } else {
        let unsupported = app
            .post(
                "/api/parse",
                &token,
                &json!({"url": "http://203.0.113.7/live"}),
            )
            .await
            .expect_status(StatusCode::OK);
        // Only managed parses carry a playback handle.
        assert!(
            unsupported.body.get("playback").is_none(),
            "{}",
            unsupported.text
        );
        assert_eq!(
            unsupported.body,
            json!({"success": false, "is_live": false, "media_info": null, "error": "Unsupported platform"})
        );
        assert_private(&unsupported);
    }

    // A batch over the limit is refused before any entry is parsed.
    let oversized: Vec<Value> = (0..101)
        .map(|index| json!({"url": format!("https://live.bilibili.com/{index}")}))
        .collect();
    let refused = app
        .post("/api/parse/batch", &token, &Value::Array(oversized))
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    // Entries fail one by one, in order, without failing the batch.
    let batch = app
        .post(
            "/api/parse/batch",
            &token,
            &json!([
                {"url": ""},
                {"url": BILIBILI, "cookies": "SESSDATA=1", "credential_id": "any"},
            ]),
        )
        .await
        .expect_status(StatusCode::OK);
    let entries = batch.body.as_array().expect("one response per entry");
    assert_eq!(entries.len(), 2, "{}", batch.text);
    assert_eq!(entries[0]["success"], false);
    assert_eq!(entries[0]["error"], "URL cannot be empty");
    assert_eq!(entries[1]["success"], false);
    assert_eq!(
        entries[1]["error"],
        "credential_id and cookies are mutually exclusive"
    );
    assert_private(&batch);

    // Resolve accepts only client-owned media, never a playback handle.
    let handle = app
        .post(
            "/api/parse/resolve",
            &token,
            &json!({"url": BILIBILI, "stream_info": {}, "playback_handle": "handle"}),
        )
        .await;
    assert_eq!(
        handle.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        handle.text
    );
    let resolved = app
        .post(
            "/api/parse/resolve",
            &token,
            &json!({"url": "", "stream_info": {}}),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        resolved.body,
        json!({"success": false, "stream_info": null, "error": "URL cannot be empty"})
    );
    assert_private(&resolved);
    let invalid = app
        .post(
            "/api/parse/resolve",
            &token,
            &json!({"url": BILIBILI, "stream_info": {"url": 5}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(invalid["success"], false);
    assert!(
        invalid["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("Invalid stream_info")),
        "{invalid}"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn parse_fails_fast_when_every_selected_account_is_disabled() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    app.select_disabled_account(&token, "platform-bilibili")
        .await;

    // The managed parse finds no usable account and stops before extracting.
    let refused = app
        .post(
            "/api/parse",
            &token,
            &json!({"url": "https://live.bilibili.com/9101"}),
        )
        .await;
    refused.assert_error(StatusCode::SERVICE_UNAVAILABLE, "CREDENTIAL_UNAVAILABLE");
    assert_eq!(
        refused.body["details"],
        json!({"reason": "profiles_disabled"})
    );
    let effective = app
        .get(
            "/api/credentials/selection?scope_type=platform&scope_id=platform-bilibili&platform_id=platform-bilibili",
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(effective["unavailable_reason"], "profiles_disabled");

    app.shutdown().await;
}

#[tokio::test]
async fn baidupcs_tools_validate_login_material_and_report_a_missing_binary() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    for body in [
        json!({"bduss": "  "}),
        json!({"cookies": "", "stoken": "only"}),
    ] {
        let refused = app.post("/api/tools/baidupcs/login", &token, &body).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    let status = app
        .post(
            "/api/tools/baidupcs/status",
            &token,
            &json!({"binary_path": "/nonexistent/BaiduPCS-Go"}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(status["resolved_binary_path"], "/nonexistent/BaiduPCS-Go");
    assert_eq!(status["binary_ok"], false);
    assert_eq!(status["logged_in"], false);
    assert_eq!(status["version"], Value::Null);
    assert_eq!(status["has_stored_credentials"], false);
    assert!(status["detail"].is_string(), "{status}");

    // The tool routes run a local binary, so a read-only key may not call them.
    let read_only = app
        .create_api_key(&token, "Dashboard", "read_only")
        .await
        .raw;
    for path in ["/api/tools/baidupcs/status", "/api/tools/baidupcs/login"] {
        let refused = app
            .post(
                path,
                &read_only,
                &json!({"binary_path": "/nonexistent/BaiduPCS-Go"}),
            )
            .await;
        refused.assert_error(StatusCode::FORBIDDEN, "API_KEY_READ_ONLY");
    }

    app.shutdown().await;
}

#[tokio::test]
async fn parse_returns_streams_for_a_live_room() {
    let fake = FakePlatform::start().await;
    let room = fake.room("parsed");
    let backup = format!("{}/backup.flv", room.url());
    room.set_live(
        room.live("Parsed show")
            .category("Music")
            .viewers(7)
            .header("Referer", "https://fake.invalid/")
            .stream(json!({
                "url": backup, "format": "flv", "media": "flv", "quality": "backup",
                "bitrate": 8_000, "priority": 1, "extras": { "headers": { "X-Edge": "b" } },
            })),
    );
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;

    let parsed = app
        .post("/api/parse", &token, &json!({ "url": room.url() }))
        .await
        .expect_status(StatusCode::OK);
    assert_private(&parsed);
    // An unmanaged parse returns the media itself, not a playback handle.
    assert!(parsed.body.get("playback").is_none(), "{}", parsed.text);
    let stream = |url: &str, quality: &str, bitrate: u64, priority: u32, extras: Value| {
        json!({
            "url": url, "stream_format": "flv", "media_format": "flv", "quality": quality,
            "bitrate": bitrate, "priority": priority, "extras": extras, "codec": "", "fps": 0.0,
            "is_audio_only": false, "is_headers_needed": false,
        })
    };
    assert_eq!(
        parsed.body,
        json!({
            "success": true,
            "is_live": true,
            "error": null,
            "media_info": {
                "site_url": room.url(),
                "title": "Parsed show",
                "artist": "parsed",
                "artist_url": null,
                "cover_url": null,
                "category": ["Music"],
                "is_live": true,
                "live_start_time": null,
                "headers": { "Referer": "https://fake.invalid/" },
                "extras": { "viewer_count": "7" },
                "streams": [
                    stream(&room.flv_url(), "origin", 16_000, 0, Value::Null),
                    stream(&backup, "backup", 8_000, 1, json!({ "headers": { "X-Edge": "b" } })),
                ],
            },
        })
    );

    // The platform's own route (direct) carried the request with the
    // platform's headers; the dead-end global route saw nothing.
    let requests = fake.requests_to(&room, "status");
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0]
            .header("user-agent")
            .is_some_and(|agent| agent.starts_with("Mozilla/5.0")),
        "{requests:?}"
    );
    assert_eq!(app.dead_end().attempts(), []);

    app.shutdown().await;
}
