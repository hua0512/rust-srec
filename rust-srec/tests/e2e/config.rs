//! Global, platform and template configuration, engines, saved proxies and
//! credential profiles, driven over HTTP as the settings pages use them.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    ClosedPort, LocalHttpServer, Response, TestApp, WAIT, assert_recent, eventually, id_of, millis,
    now_ms,
};

async fn create_profile(app: &TestApp, token: &str, body: Value) -> Value {
    app.post("/api/credentials/profiles", token, &body)
        .await
        .expect_status(StatusCode::OK)
        .body
}

fn bilibili_profile(label: &str) -> Value {
    json!({
        "platform_id": "platform-bilibili",
        "label": label,
        "enabled": true,
        "material": {"cookies": format!("SESSDATA={label}")},
    })
}

async fn streamer_total(app: &TestApp, token: &str) -> u64 {
    app.get("/api/streamers", token)
        .await
        .expect_status(StatusCode::OK)
        .body["total"]
        .as_u64()
        .expect("streamer list total")
}

async fn template_total(app: &TestApp, token: &str) -> u64 {
    app.get("/api/templates", token)
        .await
        .expect_status(StatusCode::OK)
        .body["total"]
        .as_u64()
        .expect("template list total")
}

/// Replaces a platform configuration with `edit` applied to its current body.
async fn put_platform(
    app: &TestApp,
    token: &str,
    platform_id: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Response {
    let mut body = app
        .get(&format!("/api/config/platforms/{platform_id}"), token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    edit(body.as_object_mut().expect("platform body is an object"));
    app.put(
        &format!("/api/config/platforms/{platform_id}"),
        token,
        &body,
    )
    .await
}

async fn effective_route(app: &TestApp, token: &str, query: &str) -> Response {
    app.get(&format!("/api/proxies/effective?{query}"), token)
        .await
}

#[tokio::test]
async fn global_config_matches_the_settings_schema_and_keeps_its_proxy_route() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let global = app
        .get("/api/config/global", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    for field in [
        "output_folder",
        "output_filename_template",
        "output_file_format",
        "default_download_engine",
        "log_filter_directive",
    ] {
        assert!(global[field].is_string(), "{field}: {global}");
    }
    for field in [
        "min_segment_size_bytes",
        "max_download_duration_secs",
        "max_part_size_bytes",
        "max_concurrent_downloads",
        "max_concurrent_uploads",
        "max_concurrent_cpu_jobs",
        "max_concurrent_io_jobs",
        "streamer_check_delay_ms",
        "offline_check_delay_ms",
        "offline_check_count",
        "pipeline_cpu_job_timeout_secs",
        "pipeline_io_job_timeout_secs",
        "pipeline_execute_timeout_secs",
        "queue_freshness_threshold_ms",
        // Retention days must be non-negative integers for the form's schema.
        "job_history_retention_days",
        "notification_event_log_retention_days",
        "output_retention_days",
    ] {
        assert!(global[field].is_u64(), "{field}: {global}");
    }
    assert!(
        global["gpu_health_probe_interval_secs"]
            .as_u64()
            .is_some_and(|seconds| seconds >= 1),
        "{global}"
    );
    for field in [
        "record_danmu",
        "output_retention_delete_files",
        "auto_thumbnail",
        "stream_proxy_allow_private_targets",
    ] {
        assert!(global[field].is_boolean(), "{field}: {global}");
    }
    for field in [
        "danmu_statistics",
        "default_extractor",
        "pipeline",
        "session_complete_pipeline",
        "paired_segment_pipeline",
    ] {
        assert!(
            global[field].is_null() || global[field].is_string(),
            "{field}: {global}"
        );
    }
    assert_eq!(
        global["output_folder"],
        app.output_dir().to_string_lossy().as_ref()
    );
    // The fresh database converts proxy settings as if no proxy variables
    // were set, and the strict route schema allows only `kind` here.
    assert_eq!(global["proxy_route"], json!({"kind": "direct"}));
    assert!(global.get("proxy_config").is_none(), "{global}");

    for (body, code) in [
        (
            json!({"proxy_route": {"kind": "inherit"}}),
            "PROXY_ROUTE_INVALID",
        ),
        (
            json!({"proxy_config": {"enabled": true, "url": "http://10.0.0.1:3128"}}),
            "PROXY_CONFIG_REPLACED",
        ),
        (
            json!({"proxy_config": "{\"enabled\":false}"}),
            "PROXY_CONFIG_REPLACED",
        ),
    ] {
        let mut body = body;
        body["record_danmu"] = json!(!global["record_danmu"].as_bool().unwrap_or(false));
        let refused = app.patch("/api/config/global", &token, &body).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, code);
    }
    let missing = app
        .patch(
            "/api/config/global",
            &token,
            &json!({"proxy_route": {"kind": "proxy", "id": "missing-proxy"}}),
        )
        .await;
    missing.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "PROXY_NOT_FOUND");
    assert_eq!(
        missing.body["details"],
        json!({"proxy_id": "missing-proxy"})
    );
    let unchanged = app
        .get("/api/config/global", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(unchanged, global, "refused patches must not apply anything");

    // A null legacy field is what older clients send for "unset".
    let system = app
        .patch(
            "/api/config/global",
            &token,
            &json!({"proxy_config": null, "proxy_route": {"kind": "system"}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(system["proxy_route"], json!({"kind": "system"}));

    // A patch that omits the route keeps the stored one.
    let kept = app
        .patch("/api/config/global", &token, &json!({"record_danmu": true}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(kept["proxy_route"], json!({"kind": "system"}));
    assert_eq!(kept["record_danmu"], true);
    let read_back = app
        .get("/api/config/global", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(read_back, kept);

    app.shutdown().await;
}

#[tokio::test]
async fn an_invalid_global_patch_names_the_field_and_applies_nothing() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let before = app
        .get("/api/config/global", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;

    for (field, value) in [
        ("job_history_retention_days", json!(-1)),
        ("output_retention_days", json!(1.5)),
        ("max_concurrent_downloads", json!(0)),
        ("max_concurrent_uploads", json!("3")),
        ("gpu_health_probe_interval_secs", json!(i64::MAX)),
        ("record_danmu", json!("true")),
        ("output_folder", json!({"path": "/x"})),
    ] {
        // Valid fields in the same body must not be applied either.
        let mut body = json!({
            "max_part_size_bytes": 12345,
            "auto_thumbnail": !before["auto_thumbnail"].as_bool().unwrap_or(true),
        });
        body[field] = value;
        let refused = app.patch("/api/config/global", &token, &body).await;
        refused.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
        assert!(
            refused.body["message"]
                .as_str()
                .is_some_and(|message| message.contains(field)),
            "{field}: {}",
            refused.text
        );
        let after = app
            .get("/api/config/global", &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(after, before, "{field}: nothing may change");
    }

    let applied = app
        .patch(
            "/api/config/global",
            &token,
            &json!({
                // Zero keeps history forever.
                "job_history_retention_days": 0,
                "output_retention_days": 30,
                "max_concurrent_cpu_jobs": 0,
                // Timeouts and probe intervals are clamped instead of refused.
                "pipeline_cpu_job_timeout_secs": 0,
                "pipeline_io_job_timeout_secs": -10,
                "gpu_health_probe_interval_secs": -5,
                "queue_freshness_threshold_ms": -1,
                // Null means "not sent".
                "output_folder": null,
                "default_extractor": null,
                "max_part_size_bytes": 12345,
            }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    let expected = |field: &str, value: Value| {
        assert_eq!(applied[field], value, "{field}: {applied}");
    };
    expected("job_history_retention_days", json!(0));
    expected("output_retention_days", json!(30));
    expected("max_concurrent_cpu_jobs", json!(0));
    expected("pipeline_cpu_job_timeout_secs", json!(1));
    expected("pipeline_io_job_timeout_secs", json!(1));
    expected("gpu_health_probe_interval_secs", json!(1));
    expected("queue_freshness_threshold_ms", json!(0));
    expected("max_part_size_bytes", json!(12345));
    expected("output_folder", before["output_folder"].clone());
    expected("default_extractor", before["default_extractor"].clone());
    let read_back = app
        .get("/api/config/global", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(read_back, applied);

    app.shutdown().await;
}

#[tokio::test]
async fn platform_configs_are_replaced_whole_but_keep_their_identity() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let platforms = app
        .get("/api/config/platforms", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    let platforms = platforms.as_array().expect("platform list");
    for name in ["bilibili", "huya", "douyu", "twitch", "streamlink"] {
        let platform = platforms
            .iter()
            .find(|platform| platform["name"] == name)
            .unwrap_or_else(|| panic!("no {name} platform in {platforms:?}"));
        assert_eq!(platform["id"], format!("platform-{name}"));
        assert_eq!(platform["proxy_route"], json!({"kind": "inherit"}));
        assert!(platform.get("cookies").is_none() && platform.get("proxy_config").is_none());
    }
    let missing = app
        .get("/api/config/platforms/platform-missing", &token)
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let original = app
        .get("/api/config/platforms/platform-huya", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    let mismatched = app
        .put("/api/config/platforms/platform-douyu", &token, &original)
        .await;
    mismatched.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    type Edit = fn(&mut serde_json::Map<String, Value>);
    let refusals: [(Edit, StatusCode, &str); 7] = [
        (
            |body| {
                body.insert("name".into(), json!("renamed"));
            },
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        ),
        (
            |body| {
                body.insert("fetch_delay_ms".into(), json!(u64::MAX));
            },
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        ),
        (
            |body| {
                body.insert("offline_check_count".into(), json!(0));
            },
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        ),
        (
            |body| {
                body.insert("offline_check_delay_ms".into(), json!(999));
            },
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        ),
        (
            |body| {
                body.insert("cookies".into(), json!("sid=abc"));
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "COOKIES_REPLACED",
        ),
        (
            |body| {
                body.insert("proxy_config".into(), json!({"enabled": true}));
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROXY_CONFIG_REPLACED",
        ),
        (
            |body| {
                body.insert("proxy_route".into(), json!({"kind": "proxy", "id": "gone"}));
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROXY_NOT_FOUND",
        ),
    ];
    for (edit, status, code) in refusals {
        let refused = put_platform(&app, &token, "platform-huya", |body| {
            // A valid change alongside the invalid one must not be applied.
            body.insert("output_folder".into(), json!("/changed"));
            edit(body);
        })
        .await;
        refused.assert_error(status, code);
        let stored = app
            .get("/api/config/platforms/platform-huya", &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(stored, original, "{code}: nothing may change");
    }
    // A blank cookie value asks for none and is accepted.
    put_platform(&app, &token, "platform-huya", |body| {
        body.insert("cookies".into(), json!("  "));
    })
    .await
    .expect_status(StatusCode::OK);

    let replaced = put_platform(&app, &token, "platform-huya", |body| {
        body.insert("fetch_delay_ms".into(), json!(1234));
        body.insert("offline_check_count".into(), json!(2));
        body.insert("offline_check_delay_ms".into(), json!(1000));
        body.insert("output_folder".into(), json!("/recordings/huya"));
        body.insert("proxy_route".into(), json!({"kind": "direct"}));
    })
    .await
    .expect_status(StatusCode::OK)
    .body;
    assert_eq!(replaced["name"], "huya");
    assert_eq!(replaced["fetch_delay_ms"], 1234);
    assert_eq!(replaced["offline_check_count"], 2);
    assert_eq!(replaced["offline_check_delay_ms"], 1000);
    assert_eq!(replaced["output_folder"], "/recordings/huya");
    assert_eq!(replaced["proxy_route"], json!({"kind": "direct"}));
    let stored = app
        .get("/api/config/platforms/platform-huya", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored, replaced);

    // PUT replaces every override, but an omitted route keeps the stored one.
    let cleared = put_platform(&app, &token, "platform-huya", |body| {
        body.remove("proxy_route");
        body.remove("output_folder");
        body.insert("fetch_delay_ms".into(), Value::Null);
    })
    .await
    .expect_status(StatusCode::OK)
    .body;
    assert_eq!(cleared["proxy_route"], json!({"kind": "direct"}));
    assert_eq!(cleared["output_folder"], Value::Null);
    assert_eq!(cleared["fetch_delay_ms"], Value::Null);
    assert_eq!(cleared["offline_check_count"], 2);

    app.shutdown().await;
}

#[tokio::test]
async fn templates_are_replaced_whole_counted_by_their_streamers_and_cloned() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    for (body, code) in [
        (json!({"name": ""}), "VALIDATION_ERROR"),
        (
            json!({"name": "Floor", "offline_check_count": 0}),
            "VALIDATION_ERROR",
        ),
        (
            json!({"name": "Floor", "offline_check_delay_ms": 999}),
            "VALIDATION_ERROR",
        ),
        (
            json!({"name": "Cookies", "cookies": "a=b"}),
            "COOKIES_REPLACED",
        ),
        (
            json!({"name": "Legacy", "proxy_config": {"enabled": true}}),
            "PROXY_CONFIG_REPLACED",
        ),
        (
            json!({"name": "Missing", "proxy_route": {"kind": "proxy", "id": "gone"}}),
            "PROXY_NOT_FOUND",
        ),
    ] {
        let refused = app.post("/api/templates", &token, &body).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, code);
    }
    assert_eq!(template_total(&app, &token).await, 0);

    let created = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Evening",
                "output_folder": "/recordings/evening",
                "output_file_format": "mp4",
                "record_danmu": true,
                "offline_check_count": 4,
                "offline_check_delay_ms": 2000,
                "platform_overrides": {"bilibili": {"quality": 10000}},
                "proxy_route": {"kind": "direct"},
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let template_id = id_of(&created);
    assert_eq!(created["usage_count"], 0);
    assert_eq!(created["proxy_route"], json!({"kind": "direct"}));
    assert_eq!(
        created["platform_overrides"],
        json!({"bilibili": {"quality": 10000}})
    );
    for field in ["created_at", "updated_at"] {
        let text = created[field].as_str().expect("timestamp string");
        // Millisecond RFC 3339 in UTC, as the template list renders it.
        assert!(text.ends_with('Z'), "{field}: {text}");
        if let Some((_, fraction)) = text.trim_end_matches('Z').split_once('.') {
            assert!(fraction.len() <= 3, "{field}: {text}");
        }
        assert_recent(millis(&created[field]));
    }

    // PUT is a full replace: omitted overrides are cleared, but the name and
    // the route are kept.
    let replaced = app
        .put(
            &format!("/api/templates/{template_id}"),
            &token,
            &json!({"output_file_format": "flv", "offline_check_count": 5}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(replaced["name"], "Evening");
    assert_eq!(replaced["output_file_format"], "flv");
    assert_eq!(replaced["offline_check_count"], 5);
    for field in [
        "output_folder",
        "record_danmu",
        "offline_check_delay_ms",
        "platform_overrides",
    ] {
        assert_eq!(replaced[field], Value::Null, "{field}: {replaced}");
    }
    assert_eq!(replaced["proxy_route"], json!({"kind": "direct"}));
    let stored = app
        .get(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored, replaced);
    let refused = app
        .put(
            &format!("/api/templates/{template_id}"),
            &token,
            &json!({"offline_check_delay_ms": 999}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let unknown = app
        .put("/api/templates/missing", &token, &json!({"name": "x"}))
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Template user",
                "url": "https://live.bilibili.com/9001",
                "template_id": template_id,
            }),
        )
        .await;
    assert_eq!(streamer["state"], "DISABLED");
    assert_eq!(streamer["template_id"], template_id.as_str());
    let stored = app
        .get(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored["usage_count"], 1);
    let listed = app
        .get("/api/templates", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed["items"][0]["id"], template_id.as_str());
    assert_eq!(listed["items"][0]["usage_count"], 1);
    let in_use = app
        .delete(&format!("/api/templates/{template_id}"), &token)
        .await;
    in_use.assert_error(StatusCode::CONFLICT, "CONFLICT");

    // A clone copies the fields and the route under a new id and name.
    let clone = app
        .post(
            &format!("/api/templates/{template_id}/clone"),
            &token,
            &json!({"new_name": "Evening copy"}),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    assert_ne!(clone["id"], template_id.as_str());
    assert_eq!(clone["name"], "Evening copy");
    assert_eq!(clone["usage_count"], 0);
    for field in ["output_file_format", "offline_check_count", "proxy_route"] {
        assert_eq!(clone[field], replaced[field], "{field}: {clone}");
    }
    for (path, body, status, code) in [
        (
            format!("/api/templates/{template_id}/clone"),
            json!({"new_name": "Evening copy"}),
            StatusCode::CONFLICT,
            "CONFLICT",
        ),
        (
            format!("/api/templates/{template_id}/clone"),
            json!({"new_name": ""}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            "/api/templates/missing/clone".to_string(),
            json!({"new_name": "Other"}),
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
    ] {
        let refused = app.post(&path, &token, &body).await;
        refused.assert_error(status, code);
    }
    assert_eq!(template_total(&app, &token).await, 2);

    app.put(
        &format!("/api/streamers/{}", id_of(&streamer)),
        &token,
        &json!({"template_id": null}),
    )
    .await
    .expect_status(StatusCode::OK);
    let deleted = app
        .delete(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(deleted["success"], true);
    let gone = app
        .get(&format!("/api/templates/{template_id}"), &token)
        .await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let unknown = app.delete("/api/templates/missing", &token).await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn a_template_stays_until_its_deleted_streamer_is_reaped() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let template = app
        .post("/api/templates", &token, &json!({"name": "Held"}))
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let template_id = id_of(&template);
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Retiring",
                "url": "https://live.bilibili.com/9002",
                "template_id": template_id,
            }),
        )
        .await;
    let streamer_id = id_of(&streamer);
    // An ended session whose session-complete pipeline was never dispatched
    // keeps the streamer's retirement outstanding, so its row stays marked
    // deleted instead of being reaped.
    let database = app.database().await;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, session_complete_dispatched) VALUES (?, ?, ?, ?, ?, 0)",
    )
    .bind("held-session")
    .bind(&streamer_id)
    .bind("Retiring")
    .bind(now - 60_000)
    .bind(now - 30_000)
    .execute(&database)
    .await
    .expect("seed an unsettled session");

    app.delete(&format!("/api/streamers/{streamer_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    let marked: Option<i64> =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM streamers WHERE id = ?")
            .bind(&streamer_id)
            .fetch_optional(&database)
            .await
            .expect("read the streamer row");
    assert_eq!(marked, Some(1), "the row stays marked while work is owed");

    let stored = app
        .get(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored["usage_count"], 0, "a deleted streamer is not a user");
    let refused = app
        .delete(&format!("/api/templates/{template_id}"), &token)
        .await;
    refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
    assert!(
        refused.body["message"]
            .as_str()
            .is_some_and(|message| message.contains("still being removed")),
        "{}",
        refused.text
    );
    app.get(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    database.close().await;

    app.shutdown().await;
}

#[tokio::test]
async fn engines_are_edited_partially_and_their_test_reports_availability() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let listed = app
        .get("/api/engines", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    let mut defaults: Vec<&str> = listed
        .as_array()
        .expect("engine list")
        .iter()
        .filter_map(|engine| engine["id"].as_str())
        .collect();
    defaults.sort_unstable();
    assert_eq!(
        defaults,
        ["default-ffmpeg", "default-mesio", "default-streamlink"]
    );

    let mesio = app
        .post(
            "/api/engines",
            &token,
            &json!({"name": "Fast mesio", "engine_type": "MESIO", "config": {"fix_flv": false}}),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let mesio_id = id_of(&mesio);
    assert_eq!(mesio["engine_type"], "MESIO");
    // The settings page parses `config` from a JSON string.
    let config: Value =
        serde_json::from_str(mesio["config"].as_str().expect("config is a JSON string"))
            .expect("config parses");
    assert_eq!(config, json!({"fix_flv": false}));
    let stored = app
        .get(&format!("/api/engines/{mesio_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored, mesio);
    let tested = app
        .get(&format!("/api/engines/{mesio_id}/test"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(tested["available"], true);
    assert!(
        tested["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "{tested}"
    );

    // PATCH changes only the fields it carries.
    let renamed = app
        .patch(
            &format!("/api/engines/{mesio_id}"),
            &token,
            &json!({"name": "Renamed mesio"}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(renamed["name"], "Renamed mesio");
    assert_eq!(renamed["config"], mesio["config"]);
    let reconfigured = app
        .patch(
            &format!("/api/engines/{mesio_id}"),
            &token,
            &json!({"config": {"fix_flv": true, "fix_hls": false}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(reconfigured["name"], "Renamed mesio");
    let stored = app
        .get(&format!("/api/engines/{mesio_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored, reconfigured);
    assert_eq!(
        serde_json::from_str::<Value>(stored["config"].as_str().unwrap_or_default())
            .expect("config parses"),
        json!({"fix_flv": true, "fix_hls": false})
    );

    let ffmpeg = app
        .post(
            "/api/engines",
            &token,
            &json!({
                "name": "Missing ffmpeg",
                "engine_type": "FFMPEG",
                "config": {"binary_path": "/nonexistent/ffmpeg"},
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let tested = app
        .get(&format!("/api/engines/{}/test", id_of(&ffmpeg)), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(tested, json!({"available": false, "version": null}));

    let bad_type = app
        .post(
            "/api/engines",
            &token,
            &json!({"name": "Lowercase", "engine_type": "mesio", "config": {}}),
        )
        .await;
    assert_eq!(bad_type.status, StatusCode::UNPROCESSABLE_ENTITY);

    app.delete(&format!("/api/engines/{mesio_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    for (method, path) in [
        (Method::GET, format!("/api/engines/{mesio_id}")),
        (Method::DELETE, format!("/api/engines/{mesio_id}")),
        (Method::GET, "/api/engines/missing/test".to_string()),
    ] {
        let missing = app.send(method, &path, Some(&token), None).await;
        missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let missing = app
        .patch("/api/engines/missing", &token, &json!({"name": "x"}))
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let listed = app
        .get("/api/engines", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed.as_array().map(Vec::len), Some(4));

    app.shutdown().await;
}

#[tokio::test]
#[ignore = "bug: an engine config that does not fit its engine_type is stored, and its test then returns 500"]
async fn known_bug_an_engine_config_that_does_not_fit_its_type_is_refused() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let created = app
        .post(
            "/api/engines",
            &token,
            &json!({"name": "Broken", "engine_type": "FFMPEG", "config": {"binary_path": 5}}),
        )
        .await;
    assert_eq!(
        created.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        created.text
    );
    let listed = app
        .get("/api/engines", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert!(
        listed
            .as_array()
            .is_some_and(|engines| engines.iter().all(|engine| engine["name"] != "Broken")),
        "{listed}"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn saved_proxies_never_return_their_password_and_report_conflicts_by_code() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    const PASSWORD: &str = "proxy-secret-password";

    let created = app
        .post(
            "/api/proxies",
            &token,
            &json!({
                "name": "Office",
                "url": "HTTP://10.20.30.40:3128/",
                "username": "recorder",
                "password": PASSWORD,
            }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    assert!(!created.text.contains(PASSWORD), "{}", created.text);
    let proxy = created.body;
    let proxy_id = id_of(&proxy);
    assert_eq!(proxy["name"], "Office");
    assert_eq!(proxy["url"], "http://10.20.30.40:3128");
    assert_eq!(proxy["scheme"], "http");
    assert_eq!(proxy["host"], "10.20.30.40");
    assert_eq!(proxy["port"], 3128);
    assert_eq!(proxy["username"], "recorder");
    assert_eq!(proxy["has_password"], true);
    assert_eq!(proxy["version"], 1);
    assert_eq!(proxy["usage_count"], 0);
    assert!(proxy.get("password").is_none());

    for (body, status, code, name) in [
        (
            json!({"name": "  office ", "url": "http://10.20.30.41:3128"}),
            StatusCode::CONFLICT,
            "PROXY_NAME_TAKEN",
            Some("Office"),
        ),
        (
            json!({
                "name": "Same exit",
                "url": "http://10.20.30.40:3128",
                "username": "recorder",
                "password": "another",
            }),
            StatusCode::CONFLICT,
            "PROXY_DUPLICATE",
            Some("Office"),
        ),
        (
            json!({"name": "Socks4", "url": "socks4://10.20.30.42:1080"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROXY_INVALID",
            None,
        ),
        (
            json!({"name": "Half login", "url": "http://10.20.30.43:3128", "username": "only"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROXY_INVALID",
            None,
        ),
        (
            json!({"name": "Inline login", "url": "http://user:pass@10.20.30.44:3128"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROXY_INVALID",
            None,
        ),
    ] {
        let refused = app.post("/api/proxies", &token, &body).await;
        refused.assert_error(status, code);
        if let Some(name) = name {
            assert_eq!(refused.body["details"], json!({"name": name}));
        }
    }

    let listed = app
        .get("/api/proxies", &token)
        .await
        .expect_status(StatusCode::OK);
    assert!(!listed.text.contains(PASSWORD));
    assert_eq!(listed.body, json!([proxy]));
    let detail = app
        .get(&format!("/api/proxies/{proxy_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    assert!(!detail.text.contains(PASSWORD));
    assert_eq!(detail.body["proxy"], proxy);
    assert_eq!(
        detail.body["references"],
        json!({"global": false, "platforms": [], "templates": [], "streamers": [], "accounts": []})
    );

    // An omitted password is kept.
    let renamed = app
        .patch(
            &format!("/api/proxies/{proxy_id}"),
            &token,
            &json!({"expected_version": 1, "name": "Office 2"}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(renamed["name"], "Office 2");
    assert_eq!(renamed["version"], 2);
    assert_eq!(renamed["username"], "recorder");
    assert_eq!(renamed["has_password"], true);
    let stale = app
        .patch(
            &format!("/api/proxies/{proxy_id}"),
            &token,
            &json!({"expected_version": 1, "name": "Lost update"}),
        )
        .await;
    stale.assert_error(StatusCode::CONFLICT, "PROXY_STALE_VERSION");
    // A null username removes the whole login.
    let anonymous = app
        .patch(
            &format!("/api/proxies/{proxy_id}"),
            &token,
            &json!({"expected_version": 2, "username": null}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(anonymous["username"], Value::Null);
    assert_eq!(anonymous["has_password"], false);
    assert_eq!(anonymous["version"], 3);

    let stale = app
        .delete(
            &format!("/api/proxies/{proxy_id}?expected_version=2"),
            &token,
        )
        .await;
    stale.assert_error(StatusCode::CONFLICT, "PROXY_STALE_VERSION");
    let deleted = app
        .delete(
            &format!("/api/proxies/{proxy_id}?expected_version=3"),
            &token,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text);
    let gone = app.get(&format!("/api/proxies/{proxy_id}"), &token).await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let listed = app
        .get("/api/proxies", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed, json!([]));

    app.shutdown().await;
}

#[tokio::test]
async fn a_proxy_in_use_cannot_be_deleted_and_the_refusal_lists_every_user() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let proxy = app
        .post(
            "/api/proxies",
            &token,
            &json!({"name": "Shared", "url": "socks5://10.9.8.7:1080"}),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let proxy_id = id_of(&proxy);
    let route = json!({"kind": "proxy", "id": proxy_id});

    // A route naming a proxy that does not exist is refused, and refused
    // writes leave nothing behind.
    let missing = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Missing route",
                "url": "https://live.bilibili.com/9010",
                "enabled": false,
                "streamer_specific_config": {"proxy_route": {"kind": "proxy", "id": "gone"}},
            }),
        )
        .await;
    missing.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "PROXY_NOT_FOUND");
    assert_eq!(missing.body["details"], json!({"proxy_id": "gone"}));
    assert_eq!(streamer_total(&app, &token).await, 0);

    app.patch("/api/config/global", &token, &json!({"proxy_route": route}))
        .await
        .expect_status(StatusCode::OK);
    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert("proxy_route".into(), route.clone());
    })
    .await
    .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({"name": "Routed template", "proxy_route": route}),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Routed streamer",
                "url": "https://live.bilibili.com/9011",
                "streamer_specific_config": {"proxy_route": route},
            }),
        )
        .await;
    assert_eq!(streamer["streamer_specific_config"]["proxy_route"], route);
    let mut account = bilibili_profile("Routed account");
    account["proxy_route"] = route.clone();
    let account = create_profile(&app, &token, account).await;
    assert_eq!(account["proxy_route"], route);

    let expected_references = json!({
        "global": true,
        "platforms": [{"id": "platform-bilibili", "name": "bilibili"}],
        "templates": [{"id": template["id"], "name": "Routed template", "being_removed": false}],
        "streamers": [{"id": streamer["id"], "name": "Routed streamer"}],
        "accounts": [{
            "id": account["id"],
            "label": "Routed account",
            "platform_id": "platform-bilibili",
            "platform_name": "bilibili",
        }],
    });
    let refused = app
        .delete(&format!("/api/proxies/{proxy_id}"), &token)
        .await;
    refused.assert_error(StatusCode::CONFLICT, "PROXY_REFERENCED");
    assert_eq!(
        refused.body["details"],
        json!({"references": expected_references})
    );
    let listed = app
        .get("/api/proxies", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed[0]["usage_count"], 5);
    let detail = app
        .get(&format!("/api/proxies/{proxy_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(detail["references"], expected_references);
    assert_eq!(detail["proxy"]["usage_count"], 5);

    app.shutdown().await;
}

#[tokio::test]
async fn the_effective_route_follows_streamer_then_template_then_platform_then_global() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let mut proxies = Vec::new();
    for (name, url) in [("A", "http://10.0.0.1:3128"), ("B", "http://10.0.0.2:3128")] {
        let proxy = app
            .post("/api/proxies", &token, &json!({"name": name, "url": url}))
            .await
            .expect_status(StatusCode::CREATED)
            .body;
        proxies.push(id_of(&proxy));
    }
    let (proxy_a, proxy_b) = (proxies[0].clone(), proxies[1].clone());
    app.patch(
        "/api/config/global",
        &token,
        &json!({"proxy_route": {"kind": "proxy", "id": proxy_a}}),
    )
    .await
    .expect_status(StatusCode::OK);
    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert(
            "proxy_route".into(),
            json!({"kind": "proxy", "id": proxy_b}),
        );
    })
    .await
    .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({"name": "Direct", "proxy_route": {"kind": "direct"}}),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let template_id = id_of(&template);
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Layered",
                "url": "https://live.bilibili.com/9020",
                "template_id": template_id,
            }),
        )
        .await;
    let streamer_id = id_of(&streamer);
    let streamer_query = format!("scope_type=streamer&scope_id={streamer_id}");

    let resolved = effective_route(&app, &token, &streamer_query)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        resolved,
        json!({"kind": "direct", "proxy": null, "source": "template"})
    );

    let set_streamer_route = |route: Value| {
        let app = &app;
        let token = &token;
        let streamer_id = &streamer_id;
        async move {
            app.put(
                &format!("/api/streamers/{streamer_id}"),
                token,
                &json!({"streamer_specific_config": {"proxy_route": route}}),
            )
            .await
            .expect_status(StatusCode::OK)
        }
    };
    set_streamer_route(json!({"kind": "system"})).await;
    let resolved = effective_route(&app, &token, &streamer_query)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        resolved,
        json!({"kind": "system", "proxy": null, "source": "streamer"})
    );

    set_streamer_route(json!({"kind": "inherit"})).await;
    app.put(
        &format!("/api/templates/{template_id}"),
        &token,
        &json!({"proxy_route": {"kind": "inherit"}}),
    )
    .await
    .expect_status(StatusCode::OK);
    let resolved = effective_route(&app, &token, &streamer_query)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        resolved,
        json!({"kind": "proxy", "proxy": {"id": proxy_b, "name": "B"}, "source": "platform"})
    );
    let template_scope = effective_route(
        &app,
        &token,
        &format!("scope_type=template&scope_id={template_id}&platform_id=platform-bilibili"),
    )
    .await
    .expect_status(StatusCode::OK)
    .body;
    assert_eq!(template_scope, resolved);

    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert("proxy_route".into(), json!({"kind": "inherit"}));
    })
    .await
    .expect_status(StatusCode::OK);
    let resolved = effective_route(&app, &token, &streamer_query)
        .await
        .expect_status(StatusCode::OK)
        .body;
    let global =
        json!({"kind": "proxy", "proxy": {"id": proxy_a, "name": "A"}, "source": "global"});
    assert_eq!(resolved, global);
    for query in [
        "scope_type=global".to_string(),
        "scope_type=platform&scope_id=platform-bilibili".to_string(),
    ] {
        let resolved = effective_route(&app, &token, &query)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(resolved, global, "{query}");
    }

    for (query, status, code) in [
        (
            "scope_type=everything",
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            "scope_type=platform",
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            "scope_type=streamer&scope_id=%20",
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            "scope_type=streamer&scope_id=missing",
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
    ] {
        let refused = effective_route(&app, &token, query).await;
        refused.assert_error(status, code);
    }

    app.shutdown().await;
}

#[tokio::test]
async fn the_proxy_check_guards_its_target_and_reports_what_the_proxy_answered() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    // The check requests the target through the proxy, and these local
    // proxies answer every request themselves, so the target address (a
    // public literal the guard accepts) is never contacted.
    const TARGET: &str = "http://1.1.1.1/";
    let open = LocalHttpServer::start(204, "").await;

    for body in [
        json!({"url": open.url("")}),
        json!({"url": open.url(""), "platform": "bilibili", "target_url": TARGET}),
        json!({"url": open.url(""), "platform": "nowhere"}),
    ] {
        let refused = app.post("/api/proxies/test", &token, &body).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    for target in [
        "ftp://1.1.1.1/",
        "http://user:secret@1.1.1.1/",
        "http://127.0.0.1:9/",
        "http://localhost:9/",
        "http://10.1.2.3/",
        "not a url",
    ] {
        let refused = app
            .post(
                "/api/proxies/test",
                &token,
                &json!({"url": open.url(""), "target_url": target}),
            )
            .await;
        refused.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    }
    assert!(open.received().is_empty(), "refused checks send nothing");

    let reached = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"url": open.url(""), "target_url": TARGET}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(reached["ok"], true, "{reached}");
    assert_eq!(reached["status"], 204);
    assert_eq!(reached["error"], Value::Null);
    assert!(reached["latency_ms"].is_u64());
    let received = open.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].method, "GET");
    assert_eq!(received[0].path, TARGET, "a forward-proxy request");

    let guarded = LocalHttpServer::start(407, "").await;
    let refused_login = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"url": guarded.url(""), "target_url": TARGET}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(refused_login["ok"], false);
    assert_eq!(refused_login["error"], "proxy_authentication_required");

    let closed = ClosedPort::reserve();
    let closed_url = closed.url("");
    let unreachable = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"url": closed_url, "target_url": TARGET}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(unreachable["ok"], false);
    assert_eq!(unreachable["error"], "connect_failed");
    assert_eq!(unreachable["status"], Value::Null);

    // Checking a saved proxy as edited keeps its saved login.
    let saved = app
        .post(
            "/api/proxies",
            &token,
            &json!({
                "name": "Saved",
                "url": guarded.url(""),
                "username": "recorder",
                "password": "saved-secret",
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let edited = LocalHttpServer::start(200, "").await;
    let checked = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"proxy_id": saved["id"], "url": edited.url(""), "target_url": TARGET}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(checked["ok"], true, "{checked}");
    use base64::Engine as _;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("recorder:saved-secret")
    );
    let received = edited.received();
    assert_eq!(received.len(), 1);
    assert!(
        received[0]
            .headers
            .iter()
            .any(|(name, value)| name == "proxy-authorization" && *value == expected),
        "{:?}",
        received[0].headers
    );
    let unknown = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"proxy_id": "missing", "target_url": TARGET}),
        )
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Private targets are refused only until the operator allows them.
    app.patch(
        "/api/config/global",
        &token,
        &json!({"stream_proxy_allow_private_targets": true}),
    )
    .await
    .expect_status(StatusCode::OK);
    let private_check = json!({"url": open.url(""), "target_url": "http://127.0.0.1:9/"});
    let private = eventually(WAIT, "the private-target opt-in", || {
        let app = &app;
        let token = &token;
        let body = &private_check;
        async move {
            let checked = app.post("/api/proxies/test", token, body).await;
            (checked.status == StatusCode::OK).then_some(checked.body)
        }
    })
    .await;
    assert_eq!(private["ok"], true, "{private}");
    let ftp = app
        .post(
            "/api/proxies/test",
            &token,
            &json!({"url": open.url(""), "target_url": "ftp://127.0.0.1/"}),
        )
        .await;
    ftp.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    app.shutdown().await;
}

#[tokio::test]
async fn account_selections_round_trip_on_streamers_and_templates() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let account = create_profile(&app, &token, bilibili_profile("Main")).await;
    let account_id = id_of(&account);
    let huya_account = create_profile(
        &app,
        &token,
        json!({
            "platform_id": "platform-huya",
            "label": "Huya",
            "enabled": true,
            "material": {"cookies": "udb=1"},
        }),
    )
    .await;

    let created = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Pooled",
                "url": "https://live.bilibili.com/9030",
                "streamer_specific_config": {
                    "credential_selection": {"mode": "pool", "credential_ids": [account_id]},
                    "record_danmu": true,
                },
            }),
        )
        .await;
    let streamer_id = id_of(&created);
    // The strict frontend schema needs every pool setting, so omitted ones
    // come back with their recommended defaults.
    let pool = json!({
        "mode": "pool",
        "credential_ids": [account_id],
        "strategy": "priority",
        "failover": true,
        "max_attempts": 3,
    });
    assert_eq!(
        created["streamer_specific_config"],
        json!({"credential_selection": pool, "record_danmu": true})
    );

    // A write that omits the selection keeps it.
    let kept = app
        .put(
            &format!("/api/streamers/{streamer_id}"),
            &token,
            &json!({"streamer_specific_config": {"record_danmu": false}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        kept["streamer_specific_config"],
        json!({"credential_selection": pool, "record_danmu": false})
    );
    let stored = app
        .get(&format!("/api/streamers/{streamer_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        stored["streamer_specific_config"],
        kept["streamer_specific_config"]
    );
    let removed = app
        .put(
            &format!("/api/streamers/{streamer_id}"),
            &token,
            &json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(removed["streamer_specific_config"], json!({}));

    let cookies = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Cookies",
                "url": "https://live.bilibili.com/9031",
                "enabled": false,
                "streamer_specific_config": {"cookies": "SESSDATA=raw"},
            }),
        )
        .await;
    cookies.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let foreign = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Foreign account",
                "url": "https://live.bilibili.com/9032",
                "enabled": false,
                "streamer_specific_config": {
                    "credential_selection": {"mode": "fixed", "credential_id": huya_account["id"]},
                },
            }),
        )
        .await;
    foreign.assert_error(StatusCode::CONFLICT, "CREDENTIAL_REFERENCE_INACCESSIBLE");
    assert_eq!(streamer_total(&app, &token).await, 1);

    // Template overrides are keyed by the stored lowercase platform name.
    let capitalised = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Capitalised",
                "platform_overrides": {"Bilibili": {"credential_selection": {"mode": "none"}}},
            }),
        )
        .await;
    capitalised.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(template_total(&app, &token).await, 0);
    let fixed = json!({"mode": "fixed", "credential_id": account_id});
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Selecting",
                "platform_overrides": {"bilibili": {"credential_selection": fixed, "quality": 1}},
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    assert_eq!(
        template["platform_overrides"],
        json!({"bilibili": {"credential_selection": fixed, "quality": 1}})
    );
    let template_id = id_of(&template);
    let replaced = app
        .put(
            &format!("/api/templates/{template_id}"),
            &token,
            &json!({"platform_overrides": {"bilibili": {"quality": 2}}}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        replaced["platform_overrides"],
        json!({"bilibili": {"credential_selection": fixed, "quality": 2}})
    );
    let stored = app
        .get(&format!("/api/templates/{template_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored["platform_overrides"], replaced["platform_overrides"]);

    app.shutdown().await;
}

#[tokio::test]
async fn profile_writes_return_the_conflict_codes_the_editor_reads() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let account = create_profile(&app, &token, bilibili_profile("Shared")).await;
    let account_id = id_of(&account);
    let fixed = json!({"mode": "fixed", "credential_id": account_id});

    // The platform carries its selection as JSON text.
    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert("credential_selection".into(), json!(fixed.to_string()));
    })
    .await
    .expect_status(StatusCode::OK);
    let kept = put_platform(&app, &token, "platform-bilibili", |body| {
        body.remove("credential_selection");
    })
    .await
    .expect_status(StatusCode::OK)
    .body;
    let stored: Value = serde_json::from_str(
        kept["credential_selection"]
            .as_str()
            .expect("an omitted selection is kept"),
    )
    .expect("selection JSON");
    assert_eq!(stored, fixed);
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Selecting template",
                "platform_overrides": {"bilibili": {"credential_selection": fixed}},
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Selecting streamer",
                "url": "https://live.bilibili.com/9040",
                "streamer_specific_config": {"credential_selection": fixed},
            }),
        )
        .await;

    let delete_profile = |version: i64| {
        let app = &app;
        let token = &token;
        let path = format!("/api/credentials/profiles/{account_id}");
        async move {
            app.send(
                Method::DELETE,
                &path,
                Some(token),
                Some(&json!({"expected_version": version})),
            )
            .await
        }
    };
    let referenced = delete_profile(1).await;
    referenced.assert_error(StatusCode::CONFLICT, "CREDENTIAL_PROFILE_REFERENCED");
    let selection = |owner: Value, name: &str| {
        json!({
            "owner": owner,
            "name": name,
            "platform_id": "platform-bilibili",
            "platform_name": "bilibili",
        })
    };
    assert_eq!(
        referenced.body["details"]["references"],
        json!({
            "selections": [
                selection(json!({"type": "platform", "platform_id": "platform-bilibili"}), "bilibili"),
                selection(json!({"type": "template", "template_id": template["id"]}), "Selecting template"),
                selection(json!({"type": "streamer", "streamer_id": streamer["id"]}), "Selecting streamer"),
            ],
            "recordings": [],
        })
    );
    let detail = app
        .get(&format!("/api/credentials/profiles/{account_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        detail["references"],
        referenced.body["details"]["references"]
    );
    assert!(!detail.to_string().contains("SESSDATA"), "{detail}");

    let stale = app
        .patch(
            &format!("/api/credentials/profiles/{account_id}"),
            &token,
            &json!({"expected_version": 0, "label": "Lost update"}),
        )
        .await;
    stale.assert_error(StatusCode::CONFLICT, "CREDENTIAL_STALE_VERSION");

    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert(
            "credential_selection".into(),
            json!(json!({"mode": "inherit"}).to_string()),
        );
    })
    .await
    .expect_status(StatusCode::OK);
    app.delete(&format!("/api/templates/{}", id_of(&template)), &token)
        .await
        .expect_status(StatusCode::OK);
    app.delete(&format!("/api/streamers/{}", id_of(&streamer)), &token)
        .await
        .expect_status(StatusCode::OK);
    let deleted = delete_profile(1).await.expect_status(StatusCode::OK).body;
    assert_eq!(deleted, json!({"deleted": true}));
    let gone = app
        .get(&format!("/api/credentials/profiles/{account_id}"), &token)
        .await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // A Streamlink account names its sites; two accounts cannot share one.
    let streamlink = |label: &str, site: &str| {
        json!({
            "platform_id": "platform-streamlink",
            "label": label,
            "enabled": true,
            "material": {"cookies": "SID=1"},
            "sites": [site],
        })
    };
    create_profile(&app, &token, streamlink("YouTube", "youtube.com")).await;
    let taken = app
        .post(
            "/api/credentials/profiles",
            &token,
            &streamlink("Second", "https://www.youtube.com/"),
        )
        .await;
    taken.assert_error(StatusCode::CONFLICT, "CREDENTIAL_SITE_TAKEN");
    assert_eq!(taken.body["details"]["site"], "youtube.com");
    assert_eq!(taken.body["details"]["label"], "YouTube");

    app.shutdown().await;
}

#[tokio::test]
async fn the_effective_account_selection_follows_streamer_then_template_then_platform() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let mut ids = Vec::new();
    for label in ["Platform", "Template", "Streamer"] {
        ids.push(id_of(
            &create_profile(&app, &token, bilibili_profile(label)).await,
        ));
    }
    let fixed = |id: &str| json!({"mode": "fixed", "credential_id": id});

    put_platform(&app, &token, "platform-bilibili", |body| {
        body.insert(
            "credential_selection".into(),
            json!(fixed(&ids[0]).to_string()),
        );
    })
    .await
    .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Layer",
                "platform_overrides": {"bilibili": {"credential_selection": fixed(&ids[1])}},
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let template_id = id_of(&template);
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Layered account",
                "url": "https://live.bilibili.com/9050",
                "template_id": template_id,
                "streamer_specific_config": {"credential_selection": fixed(&ids[2])},
            }),
        )
        .await;
    let streamer_id = id_of(&streamer);
    let selection_of = |scope: &str| {
        let app = &app;
        let token = &token;
        let path = format!("/api/credentials/selection?{scope}&platform_id=platform-bilibili");
        async move {
            app.get(&path, token)
                .await
                .expect_status(StatusCode::OK)
                .body
        }
    };
    let streamer_scope = format!("scope_type=streamer&scope_id={streamer_id}");
    let candidate_ids = |effective: &Value| -> Vec<String> {
        effective["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .map(|candidate| {
                candidate["profile"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };

    let effective = selection_of(&streamer_scope).await;
    assert_eq!(effective["configured"], fixed(&ids[2]));
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "streamer", "streamer_id": streamer_id})
    );
    assert_eq!(effective["resolved"]["selection"], fixed(&ids[2]));
    assert_eq!(candidate_ids(&effective), [ids[2].clone()]);
    assert_eq!(effective["unavailable_reason"], Value::Null);
    assert!(!effective.to_string().contains("SESSDATA"), "{effective}");

    app.put(
        &format!("/api/streamers/{streamer_id}"),
        &token,
        &json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}}}),
    )
    .await
    .expect_status(StatusCode::OK);
    let effective = selection_of(&streamer_scope).await;
    assert_eq!(effective["configured"], Value::Null);
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "template", "template_id": template_id})
    );
    assert_eq!(candidate_ids(&effective), [ids[1].clone()]);
    let template_scope = selection_of(&format!("scope_type=template&scope_id={template_id}")).await;
    assert_eq!(template_scope["configured"], fixed(&ids[1]));

    app.put(
        &format!("/api/templates/{template_id}"),
        &token,
        &json!({"platform_overrides": {"bilibili": {"credential_selection": {"mode": "inherit"}}}}),
    )
    .await
    .expect_status(StatusCode::OK);
    let effective = selection_of(&streamer_scope).await;
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "platform", "platform_id": "platform-bilibili"})
    );
    assert_eq!(candidate_ids(&effective), [ids[0].clone()]);
    let platform_scope = selection_of("scope_type=platform&scope_id=platform-bilibili").await;
    assert_eq!(platform_scope["configured"], fixed(&ids[0]));

    for scope in [
        "scope_type=account&scope_id=x",
        "scope_type=streamer&scope_id=%20",
    ] {
        let refused = app
            .get(
                &format!("/api/credentials/selection?{scope}&platform_id=platform-bilibili"),
                &token,
            )
            .await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    app.delete(&format!("/api/streamers/{streamer_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    let removed = app
        .get(
            &format!("/api/credentials/selection?{streamer_scope}&platform_id=platform-bilibili"),
            &token,
        )
        .await;
    removed.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn account_capabilities_and_checks_answer_without_a_provider_request() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let capabilities = |platform: &str| {
        let app = &app;
        let token = &token;
        let path = format!("/api/credentials/capabilities?platform_id={platform}");
        async move { app.get(&path, token).await }
    };
    let twitch = capabilities("platform-twitch")
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        twitch,
        json!({
            "refresh_token": false,
            "access_token": true,
            "token_only": true,
            "reauth_login": false,
            "qr_login": false,
            "check": true,
            "refresh": false,
            "per_streamer_selection": false,
        })
    );
    let soop = capabilities("platform-soop")
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(soop["reauth_login"], true);
    assert_eq!(soop["per_streamer_selection"], false);
    let huya = capabilities("platform-huya")
        .await
        .expect_status(StatusCode::OK)
        .body;
    let fields = huya.as_object().expect("capabilities object");
    assert_eq!(fields.len(), 8, "{huya}");
    assert!(
        fields.values().all(|value| value.as_bool() == Some(false)),
        "{huya}"
    );
    let streamlink = capabilities("platform-streamlink")
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(streamlink["per_streamer_selection"], true);
    capabilities("%20")
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    capabilities("platform-missing")
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Without an access token or an auth-token cookie there is nothing for
    // Twitch to check, so the check fails before any request.
    let unverifiable = create_profile(
        &app,
        &token,
        json!({
            "platform_id": "platform-twitch",
            "label": "Cookie only",
            "enabled": true,
            "material": {"cookies": "unique_id=abc"},
        }),
    )
    .await;
    let unverifiable_id = id_of(&unverifiable);
    let refused = app
        .post(
            &format!("/api/credentials/profiles/{unverifiable_id}/validate"),
            &token,
            &json!({}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        refused.body["message"]
            .as_str()
            .is_some_and(|message| message.contains("nothing the provider can check")),
        "{}",
        refused.text
    );
    let detail = app
        .get(
            &format!("/api/credentials/profiles/{unverifiable_id}"),
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        detail["health"],
        Value::Null,
        "an unverifiable check records nothing"
    );

    let disabled = create_profile(
        &app,
        &token,
        json!({
            "platform_id": "platform-twitch",
            "label": "Disabled",
            "enabled": false,
            "material": {"cookies": "auth-token=abc"},
        }),
    )
    .await;
    for action in ["validate", "refresh"] {
        let refused = app
            .post(
                &format!("/api/credentials/profiles/{}/{action}", id_of(&disabled)),
                &token,
                &json!({}),
            )
            .await;
        refused.assert_error(StatusCode::CONFLICT, "CREDENTIAL_PROFILE_DISABLED");
    }

    // A profile its provider cannot refresh is returned unchanged.
    let huya_profile = create_profile(
        &app,
        &token,
        json!({
            "platform_id": "platform-huya",
            "label": "Huya",
            "enabled": true,
            "material": {"cookies": "udb_uid=1"},
        }),
    )
    .await;
    let refreshed = app
        .post(
            &format!("/api/credentials/profiles/{}/refresh", id_of(&huya_profile)),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(refreshed["profile"]["version"], huya_profile["version"]);
    assert_eq!(refreshed["profile"]["revision"], huya_profile["revision"]);
    assert_eq!(
        refreshed["capabilities"],
        json!({"validate": false, "refresh": false, "qr_login": false})
    );
    assert_eq!(refreshed["health"], Value::Null);

    app.shutdown().await;
}

#[tokio::test]
async fn polling_a_login_answers_from_its_stored_state_without_the_provider() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let database = app.database().await;
    let admin_id: String = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&database)
        .await
        .expect("admin user id");
    let now = now_ms();
    let target =
        json!({"type": "create", "platform_id": "platform-bilibili", "label": "QR"}).to_string();
    for (id, principal, state, profile, version) in [
        ("login-pending", admin_id.as_str(), "pending", None, None),
        (
            "login-completed",
            admin_id.as_str(),
            "completed",
            Some("profile-from-qr"),
            Some(3_i64),
        ),
        ("login-other-user", "another-user", "pending", None, None),
    ] {
        sqlx::query(
            "INSERT INTO credential_login_sessions (id, principal, platform_config_id, target, provider_auth_code, created_at, expires_at, state, result_profile_id, result_version, completed_at) VALUES (?, ?, 'platform-bilibili', ?, 'auth-code', ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(principal)
        .bind(&target)
        .bind(now - 120_000)
        // Expired a moment ago, well inside the receipt window.
        .bind(now - 1_000)
        .bind(state)
        .bind(profile)
        .bind(version)
        .bind(profile.map(|_| now - 2_000))
        .execute(&database)
        .await
        .expect("seed a login session");
    }

    let poll = |id: &str| {
        let app = &app;
        let token = &token;
        let path = format!("/api/credentials/login-sessions/{id}/poll");
        async move { app.send(Method::POST, &path, Some(token), None).await }
    };
    let expired = poll("login-pending").await.expect_status(StatusCode::OK);
    assert_eq!(
        expired.body,
        json!({"status": "expired", "profile_id": null, "version": null})
    );
    assert_eq!(
        expired.header(reqwest::header::CACHE_CONTROL),
        Some("private, no-store")
    );
    let expired_again = poll("login-pending").await;
    assert_eq!(expired_again.body["status"], "expired");
    let completed = poll("login-completed").await.expect_status(StatusCode::OK);
    assert_eq!(
        completed.body,
        json!({"status": "completed", "profile_id": "profile-from-qr", "version": 3})
    );
    assert_eq!(
        completed.header(reqwest::header::CACHE_CONTROL),
        Some("private, no-store")
    );
    for id in ["login-other-user", "login-missing"] {
        poll(id)
            .await
            .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    database.close().await;

    app.shutdown().await;
}
