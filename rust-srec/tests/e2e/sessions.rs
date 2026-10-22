//! Recording sessions, their segments and danmu statistics, and the media
//! outputs they produced.
//!
//! No offline API creates sessions or outputs, so each test seeds them through
//! [`TestApp::run_sql`] after boot; the handlers read them from the database
//! on every request. Streamers that own sessions are disabled.

use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{TestApp, id_of, millis, now_ms};

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("valid timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A SQL string literal, or `NULL`.
fn literal(value: Option<&str>) -> String {
    value.map_or_else(
        || "NULL".to_string(),
        |value| format!("'{}'", value.replace('\'', "''")),
    )
}

fn number(value: Option<i64>) -> String {
    value.map_or_else(|| "NULL".to_string(), |value| value.to_string())
}

/// A `live_sessions` row. Its final pipeline is recorded as dispatched, so an
/// ended one never holds up deleting its streamer.
struct Session<'a> {
    id: &'a str,
    streamer_id: Option<&'a str>,
    streamer_name: Option<&'a str>,
    start: i64,
    end: Option<i64>,
    titles: Option<&'a str>,
    total_size_bytes: i64,
}

/// A session with no streamer and no titles, still live when `end` is `None`.
fn session(id: &str, start: i64, end: Option<i64>, total_size_bytes: i64) -> Session<'_> {
    Session {
        id,
        streamer_id: None,
        streamer_name: None,
        start,
        end,
        titles: None,
        total_size_bytes,
    }
}

impl<'a> Session<'a> {
    fn of(self, streamer_id: &'a str, streamer_name: &'a str) -> Self {
        Self {
            streamer_id: Some(streamer_id),
            streamer_name: Some(streamer_name),
            ..self
        }
    }

    fn titled(self, titles: &'a str) -> Self {
        Self {
            titles: Some(titles),
            ..self
        }
    }

    fn sql(&self) -> String {
        format!(
            "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
                 titles, total_size_bytes, session_complete_dispatched) \
             VALUES ({}, {}, {}, {}, {}, {}, {}, 1);\n",
            literal(Some(self.id)),
            literal(self.streamer_id),
            literal(self.streamer_name),
            self.start,
            number(self.end),
            literal(self.titles),
            self.total_size_bytes,
        )
    }
}

/// A `media_outputs` row per `(id, session_id, path, file_type, size, created_at)`.
fn outputs(rows: &[(&str, &str, &str, &str, i64, i64)]) -> String {
    rows.iter()
        .map(|&(id, session_id, path, file_type, size, created)| {
            format!(
                "INSERT INTO media_outputs (id, session_id, file_path, file_type, size_bytes, \
                     created_at) VALUES ({}, {}, {}, {}, {size}, {created});\n",
                literal(Some(id)),
                literal(Some(session_id)),
                literal(Some(path)),
                literal(Some(file_type)),
            )
        })
        .collect()
}

async fn get_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.get(path, token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// Create a disabled streamer and return its id.
async fn disabled_streamer(app: &TestApp, token: &str, name: &str, url: &str) -> String {
    let streamer = json!({ "name": name, "url": url });
    id_of(&app.create_disabled_streamer(token, streamer).await)
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| item["id"].as_str().expect("id").to_string())
        .collect()
}

fn item<'a>(page: &'a Value, id: &str) -> &'a Value {
    page["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {page}"))
}

/// Panics unless each field of `expected` has that value in `actual`; an
/// expected `null` also matches an absent field.
#[track_caller]
fn assert_fields(actual: &Value, expected: Value) {
    for (key, value) in expected.as_object().expect("expected fields") {
        assert_eq!(&actual[key], value, "{key} in {actual}");
    }
}

#[tokio::test]
async fn session_list_and_detail_describe_each_recording_even_after_its_streamer_is_deleted() {
    // Bob owns a session that is still live, so his row is seeded before
    // boot: a streamer created disabled over HTTP has its active session ended
    // by an asynchronous cleanup, which may run after the session is inserted.
    let app = TestApp::start_seeded(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, priority) \
         VALUES ('bob', 'Bob', 'https://live.bilibili.com/12', 'platform-bilibili', 'DISABLED', 'NORMAL');",
    )
    .await;
    let token = app.admin_token().await;
    let alice = disabled_streamer(&app, &token, "Alice", "https://live.bilibili.com/11").await;

    let now = now_ms();
    let t0 = now - 3 * 3_600_000;
    let titles = format!(
        r#"[{{"ts":{t0},"title":"Morning stream"}},{{"ts":{},"title":"Evening chat"}}]"#,
        t0 + 600_000
    );
    let started =
        json!({ "kind": "session_started", "from_hysteresis": false, "title": "Morning stream" });
    let ended = json!({ "kind": "session_ended", "cause": { "type": "completed" }, "via_hysteresis": false });
    let recorded_as = "Alice (as recorded)";
    let t2 = t0 + 2 * 3_600_000;
    let mut sql: String = [
        session("titled", t0, Some(t0 + 3_600_000), 3_000)
            .of(&alice, recorded_as)
            .titled(&titles),
        session("untitled", t2, Some(t2 + 60_000), 500).of(&alice, recorded_as),
        // Ended inside maintenance's 5-minute empty-session retention, so its
        // sweep leaves it.
        session("empty", now - 1_800_000, Some(now - 1_000), 0).of(&alice, recorded_as),
        session("bob-earlier", t0 - 3_600_000, Some(t0 - 1_800_000), 100)
            .of("bob", "Bob")
            .titled(r#"[{"ts":0,"title":"Old show"}]"#),
        session("bob-live", now - 600_000, None, 0).of("bob", "Bob"),
    ]
    .iter()
    .map(Session::sql)
    .collect();
    let titled_output =
        |id, path, file_type, size, offset| (id, "titled", path, file_type, size, t0 + offset);
    sql.push_str(&outputs(&[
        titled_output("thumb", "/media/titled.jpg", "THUMBNAIL", 10, 500),
        titled_output("video-1", "/media/titled-1.flv", "VIDEO", 1_000, 1_000),
        titled_output("video-2", "/media/titled-2.flv", "VIDEO", 1_990, 2_000),
        titled_output("chat", "/media/titled.xml", "DANMU_XML", 0, 3_000),
    ]));
    // Inserted out of order: the timeline sorts by `occurred_at`.
    sql.push_str(&format!(
        "INSERT INTO session_events (session_id, streamer_id, kind, occurred_at, payload) VALUES \
         ('titled', '{alice}', 'session_ended', {}, '{ended}'), \
         ('titled', '{alice}', 'session_started', {t0}, '{started}');\n\
         INSERT INTO danmu_statistics (id, session_id, total_danmus) VALUES ('stats', 'titled', 42);\n",
        t0 + 3_600_000
    ));
    app.run_sql(&sql).await;

    // Newest first; the zero-byte ended session is hidden by default.
    let page = get_ok(&app, &token, "/api/sessions").await;
    assert_eq!(page["total"], 4);
    assert_eq!(
        ids(&page),
        ["bob-live", "untitled", "titled", "bob-earlier"]
    );

    let titled = item(&page, "titled");
    assert_fields(
        titled,
        json!({
            "streamer_id": alice, "streamer_name": "Alice", "title": "Evening chat", "events": [],
            "is_live": false, "duration_secs": 3_600, "output_count": 4, "total_size_bytes": 3_000,
            "danmu_count": 42, "thumbnail_url": "/api/media/thumb/content",
        }),
    );
    let title_list: Vec<(&str, i64)> = titled["titles"]
        .as_array()
        .expect("titles")
        .iter()
        .map(|entry| {
            (
                entry["title"].as_str().expect("title"),
                millis(&entry["timestamp"]),
            )
        })
        .collect();
    assert_eq!(
        title_list,
        [("Morning stream", t0), ("Evening chat", t0 + 600_000)]
    );
    assert_eq!(millis(&titled["start_time"]), t0);
    assert_eq!(millis(&titled["end_time"]), t0 + 3_600_000);
    assert_fields(
        item(&page, "untitled"),
        json!({
            "title": "", "titles": [], "output_count": 0, "duration_secs": 60,
            "danmu_count": null, "thumbnail_url": null,
        }),
    );
    let live = item(&page, "bob-live");
    assert_eq!(live["is_live"], true);
    assert!(
        live["end_time"].is_null() && live["duration_secs"].is_null(),
        "{live}"
    );

    let window = format!(
        "from_date={}&to_date={}",
        rfc3339(t0),
        rfc3339(t0 + 3_600_000)
    );
    let cases: [(&str, &[&str]); 7] = [
        (
            "include_empty=true",
            &["bob-live", "empty", "untitled", "titled", "bob-earlier"],
        ),
        ("active_only=true", &["bob-live"]),
        ("streamer_id=bob", &["bob-live", "bob-earlier"]),
        ("search=EVENING", &["titled"]),
        ("search=alice", &["untitled", "titled"]),
        (&window, &["titled"]),
        ("limit=2&offset=1", &["untitled", "titled"]),
    ];
    for (query, expected) in cases {
        let page = get_ok(&app, &token, &format!("/api/sessions?{query}")).await;
        assert_eq!(ids(&page), expected, "{query}");
    }
    let paged = get_ok(&app, &token, "/api/sessions?limit=2&offset=1").await;
    assert_fields(&paged, json!({ "total": 4, "limit": 2, "offset": 1 }));

    // The detail adds the lifecycle timeline, oldest first.
    let detail = get_ok(&app, &token, "/api/sessions/titled").await;
    let events = detail["events"].as_array().expect("events");
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["session_started", "session_ended"]);
    assert_eq!(events[0]["payload"], started);
    assert_eq!(events[1]["payload"], ended);
    assert_eq!(millis(&events[0]["occurred_at"]), t0);
    for key in [
        "title",
        "output_count",
        "total_size_bytes",
        "danmu_count",
        "duration_secs",
        "thumbnail_url",
        "streamer_name",
    ] {
        assert_eq!(
            detail[key], titled[key],
            "list and detail disagree on {key}"
        );
    }
    app.get("/api/sessions/no-such-session", &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Deleting the streamer keeps its history under the recorded name, and
    // that name stays searchable.
    app.delete(&format!("/api/streamers/{alice}"), &token)
        .await
        .expect_status(StatusCode::OK);
    let orphan = json!({ "streamer_id": null, "streamer_name": recorded_as });
    let orphaned = get_ok(&app, &token, "/api/sessions/titled").await;
    assert_fields(&orphaned, orphan.clone());
    assert_eq!(orphaned["total_size_bytes"], 3_000);
    let found = get_ok(&app, &token, "/api/sessions?search=as%20recorded").await;
    assert_eq!(ids(&found), ["untitled", "titled"]);
    assert_fields(item(&found, "titled"), orphan);

    app.shutdown().await;
}

#[tokio::test]
async fn media_content_is_served_with_a_query_token_and_as_an_attachment() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let contents: &[u8] = b"\xFF\xD8\xFF\xE0 not really a jpeg";
    let cover = app.write_output_file("cover.jpg", contents);
    let vanished = app.output_path("vanished.jpg");
    let now = now_ms();
    let mut sql = session("covered", now - 600_000, Some(now - 60_000), 1_000).sql();
    let image = |id, path, size, ago| (id, "covered", path, "THUMBNAIL", size, now - ago);
    sql.push_str(&outputs(&[
        image("cover", cover.as_str(), contents.len() as i64, 300_000),
        image("vanished", vanished.as_str(), 5, 200_000),
    ]));
    app.run_sql(&sql).await;
    let fetch = async |path: &str, authorization: Option<&str>| {
        let mut request = app.client().get(app.url(path));
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        }
        request.send().await.expect("fetch media")
    };

    // The frontend appends the token to the thumbnail URL the session carries.
    let thumbnail = get_ok(&app, &token, "/api/sessions/covered").await["thumbnail_url"]
        .as_str()
        .expect("thumbnail_url")
        .to_string();
    assert_eq!(thumbnail, "/api/media/cover/content");
    let with_token = format!("{thumbnail}?token={token}");
    let response = fetch(&with_token, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("content-disposition").is_none());
    assert_eq!(
        response.bytes().await.expect("thumbnail bytes").as_ref(),
        contents
    );

    let response = fetch(&format!("{with_token}&download=1"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"cover.jpg\"; filename*=UTF-8''cover.jpg")
    );

    // A header always wins over the query token, so a bad one is refused.
    let unauthenticated = fetch(&thumbnail, None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let bad_header = fetch(&with_token, Some("Bearer not-a-token")).await;
    assert_eq!(bad_header.status(), StatusCode::UNAUTHORIZED);

    for id in ["vanished", "no-such-output"] {
        let response = fetch(&format!("/api/media/{id}/content?token={token}"), None).await;
        let response = TestApp::read(response).await;
        response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    app.shutdown().await;
}

#[tokio::test]
async fn segments_are_listed_newest_first_and_danmu_statistics_404_until_recorded() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let t = now_ms() - 3_600_000;
    let mut sql = session("segmented", t, Some(t + 600_000), 3_000).sql();
    sql.push_str(&format!(
        "INSERT INTO session_segments (id, session_id, segment_index, file_path, duration_secs, \
             size_bytes, split_reason_code, split_reason_details_json, created_at, completed_at, \
             persisted_at) VALUES \
         ('seg-0', 'segmented', 0, '/media/seg-0.flv', 60.5, 1000, 'size_limit', \
             '{{\"limit_bytes\":1000}}', {t}, {}, {}), \
         ('seg-1', 'segmented', 1, '/media/seg-1.flv', 30.0, 1500, NULL, NULL, {}, NULL, {}), \
         ('legacy', 'segmented', 2, '/media/legacy.flv', 10.0, 500, 'unknown', '{{oops', NULL, NULL, {});\n",
        t + 60_000,
        t + 61_000,
        t + 60_000,
        t + 121_000,
        t + 200_000,
    ));
    app.run_sql(&sql).await;

    let page = get_ok(&app, &token, "/api/sessions/segmented/segments").await;
    assert_fields(&page, json!({ "limit": 20, "offset": 0 }));
    assert_eq!(ids(&page), ["seg-1", "seg-0", "legacy"]);
    let first = item(&page, "seg-0");
    assert_fields(
        first,
        json!({
            "session_id": "segmented", "segment_index": 0, "file_path": "/media/seg-0.flv",
            "duration_secs": 60.5, "size_bytes": 1_000, "split_reason_code": "size_limit",
            "split_reason_details": { "limit_bytes": 1000 },
        }),
    );
    assert_eq!(millis(&first["created_at"]), t);
    assert_eq!(millis(&first["completed_at"]), t + 60_000);
    assert_eq!(millis(&first["persisted_at"]), t + 61_000);
    // `SessionSegmentSchema.created_at` is `.nullable()`, not `.optional()`:
    // the key must be present even when unknown.
    let legacy = item(&page, "legacy");
    let keys = legacy.as_object().expect("segment object");
    assert!(
        keys.contains_key("created_at") && legacy["created_at"].is_null(),
        "{legacy}"
    );
    assert!(legacy["split_reason_details"].is_null());
    assert!(legacy["completed_at"].is_null());

    let second = get_ok(
        &app,
        &token,
        "/api/sessions/segmented/segments?limit=1&offset=1",
    )
    .await;
    assert_eq!(ids(&second), ["seg-0"]);
    assert_fields(&second, json!({ "limit": 1, "offset": 1 }));
    let page = get_ok(&app, &token, "/api/sessions/segmented/segments?limit=500").await;
    assert_eq!(page["limit"], 100);

    // An unknown session is a clean 404, and so is a session's statistics row
    // before one is recorded.
    for path in [
        "/api/sessions/no-such-session/segments",
        "/api/sessions/segmented/danmu-statistics",
        "/api/sessions/no-such-session/danmu-statistics",
    ] {
        app.get(path, &token)
            .await
            .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    app.run_sql(&format!(
            "INSERT INTO danmu_statistics (id, session_id, total_danmus, danmu_rate_timeseries, \
                 top_talkers, word_frequency, unique_talkers, top_gifters, top_gifts, chat_count, \
                 gift_count, duration_secs, start_time, end_time, rate_bucket_secs) VALUES \
             ('stats', 'segmented', 9, '[{{\"ts\":{t},\"count\":3}}]', \
                 '[{{\"user_id\":\"u1\",\"username\":\"Ann\",\"message_count\":5}}]', \
                 '[{{\"word\":\"b\",\"count\":2}},{{\"word\":\"a\",\"count\":2,\"error\":1}},{{\"word\":\"c\",\"count\":5}}]', \
                 2, NULL, '[{{\"name\":\"Rocket\",\"count\":1}}]', 8, 1, 120, {t}, {}, 60);",
            t + 120_000
    ))
    .await;
    let stats = get_ok(&app, &token, "/api/sessions/segmented/danmu-statistics").await;
    assert_fields(
        &stats,
        json!({
            "session_id": "segmented", "total_danmus": 9, "unique_talkers": 2, "chat_count": 8,
            "gift_count": 1, "duration_secs": 120, "rate_bucket_secs": 60,
            "danmu_rate_timeseries": [{ "ts": t, "count": 3 }],
            "top_talkers": [{ "user_id": "u1", "username": "Ann", "message_count": 5, "error": 0 }],
            "top_gifters": [],
            "top_gifts": [{ "name": "Rocket", "count": 1 }],
            // By count, then by word.
            "word_frequency": [
                { "word": "c", "count": 5, "error": 0 },
                { "word": "a", "count": 2, "error": 1 },
                { "word": "b", "count": 2, "error": 0 },
            ],
        }),
    );
    assert_eq!(millis(&stats["start_time"]), t);
    let detail = get_ok(&app, &token, "/api/sessions/segmented").await;
    assert_eq!(detail["danmu_count"], 9);

    app.shutdown().await;
}

#[tokio::test]
async fn deleting_a_session_removes_its_records_but_leaves_its_files() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let video = app.write_output_file("doomed.flv", b"flv bytes");
    let kept_video = app.write_output_file("kept.flv", b"other flv bytes");
    let t = now_ms() - 3_600_000;
    let mut sql: String = ["doomed", "kept", "batch-1", "batch-2", "batch-3"]
        .into_iter()
        .zip([9, 15, 1, 1, 1])
        .map(|(id, size)| session(id, t, Some(t + 600_000), size).sql())
        .collect();
    sql.push_str(&outputs(&[
        ("doomed-video", "doomed", &video, "VIDEO", 9, t + 1_000),
        ("kept-video", "kept", &kept_video, "VIDEO", 15, t + 1_000),
    ]));
    sql.push_str(&format!(
        "INSERT INTO session_segments (id, session_id, segment_index, file_path, duration_secs, \
             size_bytes, created_at, persisted_at) \
         VALUES ('doomed-seg', 'doomed', 0, '{}', 10.0, 9, {t}, {t});\n\
         INSERT INTO session_events (session_id, streamer_id, kind, occurred_at, payload) \
         VALUES ('doomed', 'some-streamer', 'session_started', {t}, NULL);\n\
         INSERT INTO danmu_statistics (id, session_id, total_danmus) VALUES ('doomed-stats', 'doomed', 3);\n",
        video.replace('\'', "''")
    ));
    app.run_sql(&sql).await;

    app.delete("/api/sessions/doomed", &token)
        .await
        .expect_status(StatusCode::OK);
    for path in [
        "/api/sessions/doomed",
        "/api/sessions/doomed/segments",
        "/api/sessions/doomed/danmu-statistics",
    ] {
        app.get(path, &token)
            .await
            .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let listed = get_ok(&app, &token, "/api/pipeline/outputs?session_id=doomed").await;
    assert_eq!(listed["total"], 0);
    assert_eq!(
        app.query_i64("SELECT COUNT(*) FROM session_events WHERE session_id = 'doomed'")
            .await,
        0,
        "the deleted session's timeline is left behind"
    );
    assert!(
        Path::new(&video).exists(),
        "the recording must stay on disk"
    );
    let kept = get_ok(&app, &token, "/api/sessions/kept").await;
    assert_eq!(kept["output_count"], 1);
    app.delete("/api/sessions/doomed", &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let batch_delete = async |ids: Value| {
        app.post("/api/sessions/batch-delete", &token, &json!({ "ids": ids }))
            .await
    };
    // Only rows that existed are counted.
    let deleted = batch_delete(json!(["batch-1", "no-such-session", "batch-2"]))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(deleted.body, json!({ "deleted": 2 }));
    app.get("/api/sessions/batch-1", &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // An oversized batch is refused whole.
    let mut too_many: Vec<String> = (0..100).map(|index| format!("missing-{index}")).collect();
    too_many.push("batch-3".to_string());
    batch_delete(json!(too_many))
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    get_ok(&app, &token, "/api/sessions/batch-3").await;
    let empty = batch_delete(json!([])).await.expect_status(StatusCode::OK);
    assert_eq!(empty.body, json!({ "deleted": 0 }));

    app.shutdown().await;
}

#[tokio::test]
async fn output_list_reports_file_types_and_uploads_and_the_summary_ignores_the_type_filter() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let alpha = disabled_streamer(&app, &token, "Alpha", "https://live.bilibili.com/21").await;
    let beta = disabled_streamer(&app, &token, "Beta", "https://live.bilibili.com/22").await;

    let base = now_ms() - 3_600_000;
    let end = Some(base + 600_000);
    let mut sql = session("alpha-session", base, end, 3_360)
        .of(&alpha, "Alpha")
        .sql();
    sql.push_str(
        &session("beta-session", base, end, 4_000)
            .of(&beta, "Beta")
            .sql(),
    );
    let part = "/media/alpha/part1.flv";
    let alpha_output = |id, file, file_type, size, second: i64| {
        (
            id,
            "alpha-session",
            file,
            file_type,
            size,
            base + second * 1_000,
        )
    };
    sql.push_str(&outputs(&[
        alpha_output("part-1", part, "VIDEO", 1_000, 1),
        alpha_output("part-2", "/media/alpha/part2.flv", "VIDEO", 2_000, 2),
        alpha_output("cover", "/media/alpha/cover.jpg", "THUMBNAIL", 10, 3),
        alpha_output("chat", "/media/alpha/chat.xml", "DANMU_XML", 50, 4),
        alpha_output("track", "/media/alpha/track.m4a", "AUDIO", 300, 5),
        (
            "beta-video",
            "beta-session",
            "/media/beta/other.flv",
            "VIDEO",
            4_000,
            base + 6_000,
        ),
    ]));
    // A retry's newer rclone result replaces its older failure; a record for
    // the same path in another session belongs to that session's output.
    sql.push_str(&format!(
        "INSERT INTO upload_records (id, job_id, streamer_id, session_id, uploader, local_path, \
             remote_path, status, size_bytes, error, created_at, updated_at, completed_at) VALUES \
         ('up-old', NULL, '{alpha}', 'alpha-session', 'rclone', '{part}', NULL, 'FAILED', 1000, \
             'timeout', {b}, {b10}, NULL), \
         ('up-new', NULL, '{alpha}', 'alpha-session', 'rclone', '{part}', 'remote:alpha/part1.flv', \
             'COMPLETED', 1000, NULL, {b}, {b20}, {b20}), \
         ('up-pcs', NULL, '{alpha}', 'alpha-session', 'baidupcs', '{part}', NULL, 'SKIPPED', NULL, \
             NULL, {b}, {b15}, NULL), \
         ('up-other', NULL, '{beta}', 'beta-session', 'rclone', '{part}', NULL, 'FAILED', NULL, \
             'other session', {b}, {b30}, NULL);\n",
        b = base,
        b10 = base + 10_000,
        b15 = base + 15_000,
        b20 = base + 20_000,
        b30 = base + 30_000,
    ));
    app.run_sql(&sql).await;
    let get =
        async |path: &str| get_ok(&app, &token, &format!("/api/pipeline/outputs{path}")).await;

    let page = get("").await;
    assert_eq!(page["total"], 6);
    assert_eq!(
        ids(&page),
        ["beta-video", "track", "chat", "cover", "part-2", "part-1"]
    );
    for (id, format) in [
        ("beta-video", "VIDEO"),
        ("track", "AUDIO"),
        ("chat", "DANMU_XML"),
        ("cover", "THUMBNAIL"),
    ] {
        assert_eq!(item(&page, id)["format"], format, "{id}");
    }
    let first_part = item(&page, "part-1");
    assert_fields(
        first_part,
        json!({
            "session_id": "alpha-session", "streamer_id": alpha, "file_path": part,
            "file_size_bytes": 1_000,
        }),
    );
    assert_eq!(millis(&first_part["created_at"]), base + 1_000);
    let uploads = first_part["uploads"].as_array().expect("uploads");
    assert_eq!(uploads.len(), 2, "{uploads:?}");
    assert_eq!(uploads[0]["uploader"], "rclone");
    assert_eq!(uploads[0]["status"], "COMPLETED");
    assert_eq!(uploads[0]["remote_path"], "remote:alpha/part1.flv");
    assert_eq!(millis(&uploads[0]["completed_at"]), base + 20_000);
    assert_eq!(uploads[1]["uploader"], "baidupcs");
    assert_eq!(uploads[1]["status"], "SKIPPED");
    // Never uploaded: the key is left out rather than sent empty.
    for id in ["part-2", "beta-video", "cover"] {
        assert!(
            !item(&page, id)
                .as_object()
                .expect("output object")
                .contains_key("uploads"),
            "{id}"
        );
    }
    assert_eq!(item(&page, "beta-video")["streamer_id"], beta.as_str());

    let videos = get("?file_type=VIDEO").await;
    assert_eq!(ids(&videos), ["beta-video", "part-2", "part-1"]);
    assert_eq!(videos["total"], 3);
    // The type picker's counts stay put while one type is selected.
    assert_eq!(
        get("/summary?file_type=VIDEO").await,
        json!({
            "total_count": 6,
            "total_size_bytes": 7_360,
            "by_type": [
                { "file_type": "AUDIO", "count": 1, "size_bytes": 300 },
                { "file_type": "DANMU_XML", "count": 1, "size_bytes": 50 },
                { "file_type": "THUMBNAIL", "count": 1, "size_bytes": 10 },
                { "file_type": "VIDEO", "count": 3, "size_bytes": 7_000 },
            ],
        })
    );
    // ...but they follow the search.
    let searched = get("?search=.FLV").await;
    assert_eq!(ids(&searched), ["beta-video", "part-2", "part-1"]);
    assert_eq!(
        get("/summary?search=.flv").await,
        json!({
            "total_count": 3,
            "total_size_bytes": 7_000,
            "by_type": [{ "file_type": "VIDEO", "count": 3, "size_bytes": 7_000 }],
        })
    );
    assert_eq!(get("?session_id=alpha-session").await["total"], 5);
    let by_streamer = get(&format!("?streamer_id={beta}")).await;
    assert_eq!(ids(&by_streamer), ["beta-video"]);
    let summary = get(&format!("/summary?streamer_id={beta}")).await;
    assert_eq!(summary["total_count"], 1);
    let limited = get("?limit=2&offset=1").await;
    assert_eq!(ids(&limited), ["track", "chat"]);
    assert_eq!(limited["total"], 6);

    // Once its streamer is deleted an output reports an empty streamer id.
    app.delete(&format!("/api/streamers/{beta}"), &token)
        .await
        .expect_status(StatusCode::OK);
    let orphaned = get("?session_id=beta-session").await;
    assert_eq!(item(&orphaned, "beta-video")["streamer_id"], "");

    app.shutdown().await;
}

#[tokio::test]
async fn deleting_an_output_removes_its_file_only_when_asked_and_shrinks_the_session() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let keep = app.write_output_file("keep.flv", b"keep");
    let remove = app.write_output_file("remove.flv", b"remove");
    let gone = app.output_path("gone.flv");
    let first = app.write_output_file("batch-1.flv", b"one");
    let second = app.write_output_file("batch-2.flv", b"two");
    let t = now_ms() - 3_600_000;
    let mut sql = session("trimmed", t, Some(t + 600_000), 3_500).sql();
    sql.push_str(&session("batched", t, Some(t + 600_000), 300).sql());
    sql.push_str(&outputs(&[
        ("keep", "trimmed", &keep, "VIDEO", 1_000, t + 1_000),
        ("remove", "trimmed", &remove, "VIDEO", 2_000, t + 2_000),
        ("gone", "trimmed", &gone, "VIDEO", 500, t + 3_000),
        ("batch-1", "batched", &first, "VIDEO", 100, t + 1_000),
        ("batch-2", "batched", &second, "VIDEO", 200, t + 2_000),
    ]));
    app.run_sql(&sql).await;
    // `(total_size_bytes, output_count)` of the session.
    let totals = async |id: &str| {
        let session = get_ok(&app, &token, &format!("/api/sessions/{id}")).await;
        (
            session["total_size_bytes"].clone(),
            session["output_count"].clone(),
        )
    };
    let delete = async |path: &str| {
        let path = format!("/api/pipeline/outputs/{path}");
        app.delete(&path, &token)
            .await
            .expect_status(StatusCode::OK)
            .body
    };

    assert_eq!(totals("trimmed").await, (json!(3_500), json!(3)));
    let kept = delete("keep?delete_file=false").await;
    assert_eq!(kept["id"], "keep");
    assert_eq!(kept["file_deleted"], false);
    assert!(Path::new(&keep).exists());
    assert_eq!(totals("trimmed").await, (json!(2_500), json!(2)));

    assert_eq!(
        delete("remove?delete_file=true").await["file_deleted"],
        true
    );
    assert!(!Path::new(&remove).exists());
    assert_eq!(totals("trimmed").await, (json!(500), json!(1)));

    // A file that is already gone does not stop the record's removal.
    assert_eq!(delete("gone?delete_file=true").await["file_deleted"], false);
    assert_eq!(totals("trimmed").await, (json!(0), json!(0)));
    app.delete("/api/pipeline/outputs/keep", &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let batch_delete = async |ids: Value| {
        let body = json!({ "ids": ids, "delete_file": true });
        app.post("/api/pipeline/outputs/batch-delete", &token, &body)
            .await
    };
    let too_many: Vec<String> = (0..=100).map(|index| format!("output-{index}")).collect();
    for ids in [json!([]), json!(["batch-1", "batch-1"]), json!(too_many)] {
        batch_delete(ids)
            .await
            .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    assert!(Path::new(&first).exists());

    // Each id succeeds or fails on its own.
    let batch = batch_delete(json!(["batch-1", "no-such-output", "batch-2"]))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(batch["requested"], 3);
    assert_eq!(batch["succeeded"], 2);
    assert_eq!(batch["failed"], 1);
    let results = batch["results"].as_array().expect("results");
    for success in [&results[0], &results[2]] {
        assert_eq!(success["success"], true);
        let keys = success.as_object().expect("result object");
        assert!(
            !keys.contains_key("code") && !keys.contains_key("error"),
            "{success}"
        );
    }
    assert_eq!(results[1]["id"], "no-such-output");
    assert_eq!(results[1]["success"], false);
    assert_eq!(results[1]["code"], "NOT_FOUND");
    assert!(!Path::new(&first).exists() && !Path::new(&second).exists());
    assert_eq!(totals("batched").await, (json!(0), json!(0)));

    app.shutdown().await;
}

#[tokio::test]
async fn interrupted_outputs_delete_only_staged_temporary_files() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let staged = app.write_output_file(
        "stream.tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11.mp4",
        b"partial",
    );
    let kept = app.write_output_file(
        "kept.tmp-0f7d6a52-3b8e-4d2c-9a61-5e4b3c2d1a00.mkv",
        b"keep me",
    );
    // Recorded as staged but not named like a temporary file: only ever kept.
    let precious = app.write_output_file("precious.mp4", b"a finished recording");
    let vanished = app.output_path("vanished.tmp-1a2b3c4d-5e6f-4a1b-8c2d-3e4f5a6b7c8d.mp4");
    let running = app.write_output_file(
        "live.tmp-2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e.mp4",
        b"still writing",
    );
    let t = now_ms() - 3_600_000;
    let mut sql = String::new();
    // (id, file, final name, job, seconds after `t` it was interrupted)
    for (id, path, output_name, job_id, interrupted_after) in [
        ("staged", &staged, "stream.mp4", "job-a", Some(3)),
        ("kept", &kept, "kept.mkv", "job-a", Some(2)),
        ("precious", &precious, "other.mp4", "job-b", Some(1)),
        ("vanished", &vanished, "vanished.mp4", "job-a", Some(4)),
        // Still owned by a live attempt, so not a leftover.
        ("running", &running, "live.mp4", "job-c", None),
    ] {
        let interrupted_at = interrupted_after.map(|seconds| t + seconds * 1_000);
        sql.push_str(&format!(
            "INSERT INTO staged_outputs (id, path, output_path, job_id, attempt_id, created_at, \
                 interrupted_at) VALUES ({}, {}, {}, '{job_id}', 'attempt', {t}, {});\n",
            literal(Some(id)),
            literal(Some(path)),
            literal(Some(&app.output_path(output_name))),
            number(interrupted_at),
        ));
    }
    app.run_sql(&sql).await;
    let path = |rest: &str| format!("/api/pipeline/interrupted-outputs{rest}");
    let delete = async |rest: &str| app.delete(&path(rest), &token).await;
    let batch_delete = async |body: Value| app.post(&path("/batch-delete"), &token, &body).await;

    // Newest interruption first; a file confirmed gone is forgotten.
    let listed = get_ok(&app, &token, &path("")).await;
    let outputs = listed["outputs"].as_array().expect("outputs");
    let listed_ids: Vec<&str> = outputs
        .iter()
        .map(|entry| entry["id"].as_str().expect("id"))
        .collect();
    assert_eq!(listed_ids, ["staged", "kept", "precious"]);
    assert_fields(
        &outputs[0],
        json!({
            "path": staged, "output_path": app.output_path("stream.mp4"), "job_id": "job-a",
            "exists": true, "size_bytes": 7,
        }),
    );
    assert_eq!(millis(&outputs[0]["interrupted_at"]), t + 3_000);
    assert_eq!(listed["total_size_bytes"], 7 + 7 + 20);
    assert_eq!(
        app.query_i64("SELECT COUNT(*) FROM staged_outputs WHERE id = 'vanished'")
            .await,
        0
    );
    let by_job = get_ok(&app, &token, &path("?job_id=job-b")).await;
    assert_eq!(by_job["outputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(by_job["outputs"][0]["id"], "precious");

    delete("/precious")
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(Path::new(&precious).exists());
    let forgotten = delete("/precious?keep_file=true")
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        forgotten.body,
        json!({ "id": "precious", "file_deleted": false })
    );
    assert!(Path::new(&precious).exists());

    let deleted = delete("/staged").await.expect_status(StatusCode::OK);
    assert_eq!(
        deleted.body,
        json!({ "id": "staged", "file_deleted": true })
    );
    assert!(!Path::new(&staged).exists());
    for id in ["staged", "running", "no-such-entry"] {
        delete(&format!("/{id}"))
            .await
            .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    assert!(Path::new(&running).exists());

    batch_delete(json!({ "ids": [] }))
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let batch = batch_delete(json!({ "ids": ["kept", "no-such-entry"], "keep_files": true }))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(batch["succeeded"], 1);
    assert_eq!(batch["failed"], 1);
    assert_eq!(batch["results"][1]["code"], "NOT_FOUND");
    assert!(Path::new(&kept).exists());
    assert_eq!(
        get_ok(&app, &token, &path("")).await,
        json!({ "outputs": [], "total_size_bytes": 0 })
    );

    app.shutdown().await;
}
