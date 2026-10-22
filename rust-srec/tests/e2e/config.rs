//! Global, platform and template configuration, engines, saved proxies and
//! credential profiles, driven over HTTP as the settings pages use them.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    ClosedPort, LocalHttpServer, Response, TestApp, assert_recent, id_of, millis, now_ms,
};

/// The `total` of a list route.
async fn total(app: &TestApp, token: &str, path: &str) -> u64 {
    app.get_ok(path, token).await["total"]
        .as_u64()
        .expect("list total")
}

async fn create_profile(app: &TestApp, token: &str, body: Value) -> Value {
    app.post("/api/credentials/profiles", token, &body)
        .await
        .ok()
}

/// An enabled cookie account.
fn profile(platform_id: &str, label: &str, cookies: &str) -> Value {
    json!({
        "platform_id": platform_id,
        "label": label,
        "enabled": true,
        "material": {"cookies": cookies},
    })
}

fn bilibili_profile(label: &str) -> Value {
    profile("platform-bilibili", label, &format!("SESSDATA={label}"))
}

/// Replaces a platform configuration with `edit` applied to its current body.
async fn put_platform(
    app: &TestApp,
    token: &str,
    platform_id: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Response {
    let path = format!("/api/config/platforms/{platform_id}");
    let mut body = app.get_ok(&path, token).await;
    edit(body.as_object_mut().expect("platform body is an object"));
    app.put(&path, token, &body).await
}

/// Replaces a platform configuration with one field changed.
async fn set_platform_field(
    app: &TestApp,
    token: &str,
    platform_id: &str,
    field: &str,
    value: Value,
) -> Value {
    put_platform(app, token, platform_id, |body| {
        body.insert(field.into(), value);
    })
    .await
    .ok()
}

/// The proxy route that applies to a scope.
async fn effective_route(app: &TestApp, token: &str, query: &str) -> Value {
    app.get_ok(&format!("/api/proxies/effective?{query}"), token)
        .await
}

#[tokio::test]
async fn global_config_matches_the_settings_schema_and_applies_a_patch_whole_or_not_at_all() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let global = app.get_ok("/api/config/global", &token).await;
    // The settings form refuses to submit negative or fractional retention
    // days, a negative freshness threshold, or a probe interval under 1 s.
    for field in [
        "job_history_retention_days",
        "notification_event_log_retention_days",
        "output_retention_days",
        "queue_freshness_threshold_ms",
    ] {
        assert!(global[field].is_u64(), "{field}: {global}");
    }
    assert!(
        global["gpu_health_probe_interval_secs"]
            .as_u64()
            .is_some_and(|seconds| seconds >= 1),
        "{global}"
    );
    assert_eq!(
        global["output_folder"],
        app.output_dir().to_string_lossy().as_ref()
    );
    // The fresh database converts proxy settings as if no proxy variables
    // were set, and the strict route schema allows only `kind` here.
    assert_eq!(global["proxy_route"], json!({"kind": "direct"}));
    assert!(global.get("proxy_config").is_none(), "{global}");

    for (mut body, code) in [
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
    assert_eq!(
        app.get_ok("/api/config/global", &token).await,
        global,
        "refused patches must not apply anything"
    );

    // An invalid field is named, and valid fields in the same body are not
    // applied either.
    for (field, value) in [
        ("job_history_retention_days", json!(-1)),
        ("output_retention_days", json!(1.5)),
        ("max_concurrent_downloads", json!(0)),
        ("max_concurrent_uploads", json!("3")),
        // Out of range everywhere; whether i64::MAX seconds fit the timer
        // depends on the host's Instant range.
        ("gpu_health_probe_interval_secs", json!(u64::MAX)),
        ("record_danmu", json!("true")),
        ("output_folder", json!({"path": "/x"})),
    ] {
        let mut body = json!({
            "max_part_size_bytes": 12345,
            "auto_thumbnail": !global["auto_thumbnail"].as_bool().unwrap_or(true),
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
        assert_eq!(
            app.get_ok("/api/config/global", &token).await,
            global,
            "{field}: nothing may change"
        );
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
        .ok();
    for (field, value) in [
        ("job_history_retention_days", json!(0)),
        ("output_retention_days", json!(30)),
        ("max_concurrent_cpu_jobs", json!(0)),
        ("pipeline_cpu_job_timeout_secs", json!(1)),
        ("pipeline_io_job_timeout_secs", json!(1)),
        ("gpu_health_probe_interval_secs", json!(1)),
        ("queue_freshness_threshold_ms", json!(0)),
        ("max_part_size_bytes", json!(12345)),
        ("output_folder", global["output_folder"].clone()),
        ("default_extractor", global["default_extractor"].clone()),
    ] {
        assert_eq!(applied[field], value, "{field}: {applied}");
    }
    assert_eq!(app.get_ok("/api/config/global", &token).await, applied);

    // A null legacy field is what older clients send for "unset".
    let system = app
        .patch(
            "/api/config/global",
            &token,
            &json!({"proxy_config": null, "proxy_route": {"kind": "system"}}),
        )
        .await
        .ok();
    assert_eq!(system["proxy_route"], json!({"kind": "system"}));
    // A patch that omits the route keeps the stored one.
    let kept = app
        .patch("/api/config/global", &token, &json!({"record_danmu": true}))
        .await
        .ok();
    assert_eq!(kept["proxy_route"], json!({"kind": "system"}));
    assert_eq!(kept["record_danmu"], true);
    assert_eq!(app.get_ok("/api/config/global", &token).await, kept);

    app.shutdown().await;
}

#[tokio::test]
async fn platform_configs_are_replaced_whole_but_keep_their_identity() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let platforms = app.get_ok("/api/config/platforms", &token).await;
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

    const HUYA: &str = "/api/config/platforms/platform-huya";
    let original = app.get_ok(HUYA, &token).await;
    let mismatched = app
        .put("/api/config/platforms/platform-douyu", &token, &original)
        .await;
    mismatched.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    let (bad, invalid) = (StatusCode::BAD_REQUEST, StatusCode::UNPROCESSABLE_ENTITY);
    for (field, value, status, code) in [
        ("name", json!("renamed"), bad, "BAD_REQUEST"),
        ("fetch_delay_ms", json!(u64::MAX), bad, "BAD_REQUEST"),
        ("offline_check_count", json!(0), bad, "BAD_REQUEST"),
        ("offline_check_delay_ms", json!(999), bad, "BAD_REQUEST"),
        ("cookies", json!("sid=abc"), invalid, "COOKIES_REPLACED"),
        (
            "proxy_config",
            json!({"enabled": true}),
            invalid,
            "PROXY_CONFIG_REPLACED",
        ),
        (
            "proxy_route",
            json!({"kind": "proxy", "id": "gone"}),
            invalid,
            "PROXY_NOT_FOUND",
        ),
    ] {
        let refused = put_platform(&app, &token, "platform-huya", |body| {
            // A valid change alongside the invalid one must not be applied.
            body.insert("output_folder".into(), json!("/changed"));
            body.insert(field.into(), value);
        })
        .await;
        refused.assert_error(status, code);
        let stored = app.get_ok(HUYA, &token).await;
        assert_eq!(stored, original, "{field}: nothing may change");
    }
    // A blank cookie value asks for none and is accepted.
    set_platform_field(&app, &token, "platform-huya", "cookies", json!("  ")).await;

    let replaced = put_platform(&app, &token, "platform-huya", |body| {
        body.insert("fetch_delay_ms".into(), json!(1234));
        body.insert("offline_check_count".into(), json!(2));
        body.insert("offline_check_delay_ms".into(), json!(1000));
        body.insert("output_folder".into(), json!("/recordings/huya"));
        body.insert("proxy_route".into(), json!({"kind": "direct"}));
    })
    .await
    .ok();
    assert_eq!(replaced["name"], "huya");
    assert_eq!(replaced["fetch_delay_ms"], 1234);
    assert_eq!(replaced["offline_check_count"], 2);
    assert_eq!(replaced["offline_check_delay_ms"], 1000);
    assert_eq!(replaced["output_folder"], "/recordings/huya");
    assert_eq!(replaced["proxy_route"], json!({"kind": "direct"}));
    assert_eq!(app.get_ok(HUYA, &token).await, replaced);

    // PUT replaces every override, but an omitted route keeps the stored one.
    let cleared = put_platform(&app, &token, "platform-huya", |body| {
        body.remove("proxy_route");
        body.remove("output_folder");
        body.insert("fetch_delay_ms".into(), Value::Null);
    })
    .await
    .ok();
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
    assert_eq!(total(&app, &token, "/api/templates").await, 0);

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
        .created();
    let template_id = id_of(&created);
    let path = format!("/api/templates/{template_id}");
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
            &path,
            &token,
            &json!({"output_file_format": "flv", "offline_check_count": 5}),
        )
        .await
        .ok();
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
    assert_eq!(app.get_ok(&path, &token).await, replaced);
    let refused = app
        .put(&path, &token, &json!({"offline_check_delay_ms": 999}))
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
    assert_eq!(app.get_ok(&path, &token).await["usage_count"], 1);
    let listed = app.get_ok("/api/templates", &token).await;
    assert_eq!(listed["items"][0]["id"], template_id.as_str());
    assert_eq!(listed["items"][0]["usage_count"], 1);
    let in_use = app.delete(&path, &token).await;
    in_use.assert_error(StatusCode::CONFLICT, "CONFLICT");

    // A clone copies the fields and the route under a new id and name.
    let clone_path = format!("{path}/clone");
    let clone = app
        .post(&clone_path, &token, &json!({"new_name": "Evening copy"}))
        .await
        .created();
    assert_ne!(clone["id"], template_id.as_str());
    assert_eq!(clone["name"], "Evening copy");
    assert_eq!(clone["usage_count"], 0);
    for field in ["output_file_format", "offline_check_count", "proxy_route"] {
        assert_eq!(clone[field], replaced[field], "{field}: {clone}");
    }
    for (path, new_name, status, code) in [
        (
            &clone_path,
            "Evening copy",
            StatusCode::CONFLICT,
            "CONFLICT",
        ),
        (
            &clone_path,
            "",
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            &"/api/templates/missing/clone".to_string(),
            "Other",
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
    ] {
        let refused = app.post(path, &token, &json!({"new_name": new_name})).await;
        refused.assert_error(status, code);
    }
    assert_eq!(total(&app, &token, "/api/templates").await, 2);

    app.put(
        &format!("/api/streamers/{}", id_of(&streamer)),
        &token,
        &json!({"template_id": null}),
    )
    .await
    .ok();
    let deleted = app.delete(&path, &token).await.ok();
    assert_eq!(deleted["success"], true);
    let gone = app.get(&path, &token).await;
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
        .created();
    let path = format!("/api/templates/{}", id_of(&template));
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Retiring",
                "url": "https://live.bilibili.com/9002",
                "template_id": template["id"],
            }),
        )
        .await;
    let streamer_id = id_of(&streamer);
    // An ended session whose session-complete pipeline was never dispatched
    // keeps the streamer's retirement outstanding, so its row stays marked
    // deleted instead of being reaped.
    let now = now_ms();
    app.run_sql(&format!(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             session_complete_dispatched) \
         VALUES ('held-session', '{streamer_id}', 'Retiring', {}, {}, 0);",
        now - 60_000,
        now - 30_000
    ))
    .await;

    app.delete(&format!("/api/streamers/{streamer_id}"), &token)
        .await
        .ok();
    let marked = app
        .query_i64(&format!(
            "SELECT COUNT(*) FROM streamers WHERE id = '{streamer_id}' AND deleted_at IS NOT NULL"
        ))
        .await;
    assert_eq!(marked, 1, "the row stays marked while work is owed");

    let stored = app.get_ok(&path, &token).await;
    assert_eq!(stored["usage_count"], 0, "a deleted streamer is not a user");
    let refused = app.delete(&path, &token).await;
    refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
    app.get_ok(&path, &token).await;

    app.shutdown().await;
}

#[tokio::test]
async fn engines_are_validated_against_their_type_edited_partially_and_tested() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let listed = app.get_ok("/api/engines", &token).await;
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

    let create = |body: Value| {
        let (app, token) = (&app, &token);
        async move { app.post("/api/engines", token, &body).await }
    };
    let mesio =
        create(json!({"name": "Fast mesio", "engine_type": "MESIO", "config": {"fix_flv": false}}))
            .await
            .created();
    let mesio_path = format!("/api/engines/{}", id_of(&mesio));
    assert_eq!(mesio["engine_type"], "MESIO");
    // The settings page parses `config` from a JSON string.
    let config_of = |engine: &Value| -> Value {
        serde_json::from_str(engine["config"].as_str().expect("config is a JSON string"))
            .expect("config parses")
    };
    assert_eq!(config_of(&mesio), json!({"fix_flv": false}));
    assert_eq!(app.get_ok(&mesio_path, &token).await, mesio);
    let tested = app.get_ok(&format!("{mesio_path}/test"), &token).await;
    assert_eq!(tested["available"], true);
    assert!(
        tested["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "{tested}"
    );

    // PATCH changes only the fields it carries.
    let renamed = app
        .patch(&mesio_path, &token, &json!({"name": "Renamed mesio"}))
        .await
        .ok();
    assert_eq!(renamed["name"], "Renamed mesio");
    assert_eq!(renamed["config"], mesio["config"]);
    let reconfigured = app
        .patch(
            &mesio_path,
            &token,
            &json!({"config": {"fix_flv": true, "fix_hls": false}}),
        )
        .await
        .ok();
    assert_eq!(reconfigured["name"], "Renamed mesio");
    let stored = app.get_ok(&mesio_path, &token).await;
    assert_eq!(stored, reconfigured);
    assert_eq!(
        config_of(&stored),
        json!({"fix_flv": true, "fix_hls": false})
    );

    let ffmpeg = create(json!({
        "name": "Missing ffmpeg",
        "engine_type": "FFMPEG",
        "config": {"binary_path": "/nonexistent/ffmpeg"},
    }))
    .await
    .created();
    let ffmpeg_test = format!("/api/engines/{}/test", id_of(&ffmpeg));
    let tested = app.get_ok(&ffmpeg_test, &token).await;
    assert_eq!(tested, json!({"available": false, "version": null}));

    // A type or config that does not fit is refused, on create and on edit.
    let bad_type = create(json!({"name": "Lowercase", "engine_type": "mesio", "config": {}})).await;
    assert_eq!(bad_type.status, StatusCode::UNPROCESSABLE_ENTITY);
    create(json!({"name": "Broken", "engine_type": "FFMPEG", "config": {"binary_path": 5}}))
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let fits =
        create(json!({"name": "Fits", "engine_type": "FFMPEG", "config": {"timeout_secs": 10}}))
            .await
            .created();
    let fits_path = format!("/api/engines/{}", id_of(&fits));
    let refused = app
        .patch(
            &fits_path,
            &token,
            &json!({"config": {"timeout_secs": "soon"}}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(
        app.get_ok(&fits_path, &token).await["config"],
        fits["config"]
    );

    app.delete(&mesio_path, &token).await.ok();
    for (method, path) in [
        (Method::GET, mesio_path.clone()),
        (Method::DELETE, mesio_path),
        (Method::GET, "/api/engines/missing/test".to_string()),
    ] {
        let missing = app.send(method, &path, Some(&token), None).await;
        missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let missing = app
        .patch("/api/engines/missing", &token, &json!({"name": "x"}))
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    // The defaults, the missing ffmpeg and the one that fits: nothing
    // refused was stored.
    let listed = app.get_ok("/api/engines", &token).await;
    assert_eq!(listed.as_array().map(Vec::len), Some(5), "{listed}");

    app.shutdown().await;
}

/// A stored config that does not fit its type still lists, can be renamed,
/// and can be replaced by one that fits.
#[tokio::test]
async fn a_stored_engine_config_that_does_not_fit_its_type_still_loads() {
    let app = TestApp::start_seeded(
        r#"INSERT INTO engine_configuration (id, name, engine_type, config)
           VALUES ('legacy', 'Legacy', 'FFMPEG', '{"binary_path":5}');"#,
    )
    .await;
    let token = app.admin_token().await;

    let listed = app.get_ok("/api/engines", &token).await;
    assert!(
        listed
            .as_array()
            .is_some_and(|engines| engines.iter().any(|engine| engine["id"] == "legacy")),
        "{listed}"
    );
    let renamed = app
        .patch("/api/engines/legacy", &token, &json!({"name": "Renamed"}))
        .await
        .ok();
    assert_eq!(renamed["name"], "Renamed");
    assert_eq!(renamed["config"], r#"{"binary_path":5}"#);
    let fixed = app
        .patch(
            "/api/engines/legacy",
            &token,
            &json!({"config": {"binary_path": "/usr/bin/ffmpeg"}}),
        )
        .await
        .ok();
    assert_eq!(fixed["config"], r#"{"binary_path":"/usr/bin/ffmpeg"}"#);

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
    let path = format!("/api/proxies/{}", id_of(&proxy));
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
    let detail = app.get(&path, &token).await.expect_status(StatusCode::OK);
    assert!(!detail.text.contains(PASSWORD));
    assert_eq!(detail.body["proxy"], proxy);
    assert_eq!(
        detail.body["references"],
        json!({"global": false, "platforms": [], "templates": [], "streamers": [], "accounts": []})
    );

    // An omitted password is kept.
    let renamed = app
        .patch(
            &path,
            &token,
            &json!({"expected_version": 1, "name": "Office 2"}),
        )
        .await
        .ok();
    assert_eq!(renamed["name"], "Office 2");
    assert_eq!(renamed["version"], 2);
    assert_eq!(renamed["username"], "recorder");
    assert_eq!(renamed["has_password"], true);
    let stale = app
        .patch(
            &path,
            &token,
            &json!({"expected_version": 1, "name": "Lost update"}),
        )
        .await;
    stale.assert_error(StatusCode::CONFLICT, "PROXY_STALE_VERSION");
    // A null username removes the whole login.
    let anonymous = app
        .patch(
            &path,
            &token,
            &json!({"expected_version": 2, "username": null}),
        )
        .await
        .ok();
    assert_eq!(anonymous["username"], Value::Null);
    assert_eq!(anonymous["has_password"], false);
    assert_eq!(anonymous["version"], 3);

    let stale = app
        .delete(&format!("{path}?expected_version=2"), &token)
        .await;
    stale.assert_error(StatusCode::CONFLICT, "PROXY_STALE_VERSION");
    let deleted = app
        .delete(&format!("{path}?expected_version=3"), &token)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text);
    let gone = app.get(&path, &token).await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(app.get_ok("/api/proxies", &token).await, json!([]));

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
        .created();
    let proxy_path = format!("/api/proxies/{}", id_of(&proxy));
    let route = json!({"kind": "proxy", "id": proxy["id"]});

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
    assert_eq!(total(&app, &token, "/api/streamers").await, 0);

    app.patch("/api/config/global", &token, &json!({"proxy_route": route}))
        .await
        .ok();
    set_platform_field(
        &app,
        &token,
        "platform-bilibili",
        "proxy_route",
        route.clone(),
    )
    .await;
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({"name": "Routed template", "proxy_route": route}),
        )
        .await
        .created();
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
    let refused = app.delete(&proxy_path, &token).await;
    refused.assert_error(StatusCode::CONFLICT, "PROXY_REFERENCED");
    assert_eq!(
        refused.body["details"],
        json!({"references": expected_references})
    );
    assert_eq!(
        app.get_ok("/api/proxies", &token).await[0]["usage_count"],
        5
    );
    let detail = app.get_ok(&proxy_path, &token).await;
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
            .created();
        proxies.push(id_of(&proxy));
    }
    let (proxy_a, proxy_b) = (proxies[0].clone(), proxies[1].clone());
    app.patch(
        "/api/config/global",
        &token,
        &json!({"proxy_route": {"kind": "proxy", "id": proxy_a}}),
    )
    .await
    .ok();
    let route_b = json!({"kind": "proxy", "id": proxy_b});
    set_platform_field(&app, &token, "platform-bilibili", "proxy_route", route_b).await;
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({"name": "Direct", "proxy_route": {"kind": "direct"}}),
        )
        .await
        .created();
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

    assert_eq!(
        effective_route(&app, &token, &streamer_query).await,
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
            .ok()
        }
    };
    set_streamer_route(json!({"kind": "system"})).await;
    assert_eq!(
        effective_route(&app, &token, &streamer_query).await,
        json!({"kind": "system", "proxy": null, "source": "streamer"})
    );

    set_streamer_route(json!({"kind": "inherit"})).await;
    app.put(
        &format!("/api/templates/{template_id}"),
        &token,
        &json!({"proxy_route": {"kind": "inherit"}}),
    )
    .await
    .ok();
    let resolved = effective_route(&app, &token, &streamer_query).await;
    assert_eq!(
        resolved,
        json!({"kind": "proxy", "proxy": {"id": proxy_b, "name": "B"}, "source": "platform"})
    );
    let template_scope = effective_route(
        &app,
        &token,
        &format!("scope_type=template&scope_id={template_id}&platform_id=platform-bilibili"),
    )
    .await;
    assert_eq!(template_scope, resolved);

    let inherit = json!({"kind": "inherit"});
    set_platform_field(&app, &token, "platform-bilibili", "proxy_route", inherit).await;
    let global =
        json!({"kind": "proxy", "proxy": {"id": proxy_a, "name": "A"}, "source": "global"});
    for query in [
        streamer_query.as_str(),
        "scope_type=global",
        "scope_type=platform&scope_id=platform-bilibili",
    ] {
        assert_eq!(
            effective_route(&app, &token, query).await,
            global,
            "{query}"
        );
    }

    for query in [
        "scope_type=everything",
        "scope_type=platform",
        "scope_type=streamer&scope_id=%20",
    ] {
        let refused = app
            .get(&format!("/api/proxies/effective?{query}"), &token)
            .await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    let refused = app
        .get(
            "/api/proxies/effective?scope_type=streamer&scope_id=missing",
            &token,
        )
        .await;
    refused.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

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
    let check = |body: Value| {
        let app = &app;
        let token = &token;
        async move { app.post("/api/proxies/test", token, &body).await }
    };

    for body in [
        json!({"url": open.url("")}),
        json!({"url": open.url(""), "platform": "bilibili", "target_url": TARGET}),
        json!({"url": open.url(""), "platform": "nowhere"}),
    ] {
        let refused = check(body).await;
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
        let refused = check(json!({"url": open.url(""), "target_url": target})).await;
        refused.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    }
    assert!(open.received().is_empty(), "refused checks send nothing");

    let reached = check(json!({"url": open.url(""), "target_url": TARGET}))
        .await
        .ok();
    assert_eq!(reached["ok"], true, "{reached}");
    assert_eq!(reached["status"], 204);
    assert_eq!(reached["error"], Value::Null);
    assert!(reached["latency_ms"].is_u64());
    let received = open.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].method, "GET");
    assert_eq!(received[0].path, TARGET, "a forward-proxy request");

    let guarded = LocalHttpServer::start(407, "").await;
    let refused_login = check(json!({"url": guarded.url(""), "target_url": TARGET}))
        .await
        .ok();
    assert_eq!(refused_login["ok"], false);
    assert_eq!(refused_login["error"], "proxy_authentication_required");

    let closed = ClosedPort::reserve();
    let unreachable = check(json!({"url": closed.url(""), "target_url": TARGET}))
        .await
        .ok();
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
        .created();
    let edited = LocalHttpServer::start(200, "").await;
    let checked =
        check(json!({"proxy_id": saved["id"], "url": edited.url(""), "target_url": TARGET}))
            .await
            .ok();
    assert_eq!(checked["ok"], true, "{checked}");
    use base64::Engine as _;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("recorder:saved-secret")
    );
    let received = edited.received();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].header("proxy-authorization"),
        Some(expected.as_str())
    );
    let unknown = check(json!({"proxy_id": "missing", "target_url": TARGET})).await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Private targets are refused only until the operator allows them.
    app.patch(
        "/api/config/global",
        &token,
        &json!({"stream_proxy_allow_private_targets": true}),
    )
    .await
    .ok();
    let private = check(json!({"url": open.url(""), "target_url": "http://127.0.0.1:9/"}))
        .await
        .ok();
    assert_eq!(private["ok"], true, "{private}");

    app.shutdown().await;
}

#[tokio::test]
async fn account_selections_round_trip_on_streamers_and_templates() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let account = create_profile(&app, &token, bilibili_profile("Main")).await;
    let account_id = id_of(&account);
    let huya_account =
        create_profile(&app, &token, profile("platform-huya", "Huya", "udb=1")).await;

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
    let path = format!("/api/streamers/{}", id_of(&created));
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
            &path,
            &token,
            &json!({"streamer_specific_config": {"record_danmu": false}}),
        )
        .await
        .ok();
    assert_eq!(
        kept["streamer_specific_config"],
        json!({"credential_selection": pool, "record_danmu": false})
    );
    assert_eq!(
        app.get_ok(&path, &token).await["streamer_specific_config"],
        kept["streamer_specific_config"]
    );
    let removed = app
        .put(
            &path,
            &token,
            &json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}}}),
        )
        .await
        .ok();
    assert_eq!(removed["streamer_specific_config"], json!({}));

    for (name, config, status, code) in [
        (
            "Cookies",
            json!({"cookies": "SESSDATA=raw"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            "Foreign account",
            json!({"credential_selection": {"mode": "fixed", "credential_id": huya_account["id"]}}),
            StatusCode::CONFLICT,
            "CREDENTIAL_REFERENCE_INACCESSIBLE",
        ),
    ] {
        let refused = app
            .post(
                "/api/streamers",
                &token,
                &json!({
                    "name": name,
                    "url": "https://live.bilibili.com/9031",
                    "enabled": false,
                    "streamer_specific_config": config,
                }),
            )
            .await;
        refused.assert_error(status, code);
    }
    assert_eq!(total(&app, &token, "/api/streamers").await, 1);

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
    assert_eq!(total(&app, &token, "/api/templates").await, 0);
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
        .created();
    assert_eq!(
        template["platform_overrides"],
        json!({"bilibili": {"credential_selection": fixed, "quality": 1}})
    );
    let template_path = format!("/api/templates/{}", id_of(&template));
    let replaced = app
        .put(
            &template_path,
            &token,
            &json!({"platform_overrides": {"bilibili": {"quality": 2}}}),
        )
        .await
        .ok();
    assert_eq!(
        replaced["platform_overrides"],
        json!({"bilibili": {"credential_selection": fixed, "quality": 2}})
    );
    assert_eq!(
        app.get_ok(&template_path, &token).await["platform_overrides"],
        replaced["platform_overrides"]
    );

    app.shutdown().await;
}

#[tokio::test]
async fn profile_writes_return_the_conflict_codes_the_editor_reads() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let account = create_profile(&app, &token, bilibili_profile("Shared")).await;
    let path = format!("/api/credentials/profiles/{}", id_of(&account));
    let fixed = json!({"mode": "fixed", "credential_id": account["id"]});

    // The platform carries its selection as JSON text.
    let selection = json!(fixed.to_string());
    set_platform_field(
        &app,
        &token,
        "platform-bilibili",
        "credential_selection",
        selection,
    )
    .await;
    let kept = put_platform(&app, &token, "platform-bilibili", |body| {
        body.remove("credential_selection");
    })
    .await
    .ok();
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
        .created();
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

    let delete_profile = || {
        let (app, token, path) = (&app, &token, &path);
        let body = json!({"expected_version": 1});
        async move {
            app.send(Method::DELETE, path, Some(token), Some(&body))
                .await
        }
    };
    let referenced = delete_profile().await;
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
    let detail = app.get_ok(&path, &token).await;
    assert_eq!(
        detail["references"],
        referenced.body["details"]["references"]
    );
    assert!(!detail.to_string().contains("SESSDATA"), "{detail}");

    let stale = app
        .patch(
            &path,
            &token,
            &json!({"expected_version": 0, "label": "Lost update"}),
        )
        .await;
    stale.assert_error(StatusCode::CONFLICT, "CREDENTIAL_STALE_VERSION");

    let inherit = json!(json!({"mode": "inherit"}).to_string());
    set_platform_field(
        &app,
        &token,
        "platform-bilibili",
        "credential_selection",
        inherit,
    )
    .await;
    app.delete(&format!("/api/templates/{}", id_of(&template)), &token)
        .await
        .ok();
    app.delete(&format!("/api/streamers/{}", id_of(&streamer)), &token)
        .await
        .ok();
    assert_eq!(delete_profile().await.ok(), json!({"deleted": true}));
    let gone = app.get(&path, &token).await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // A Streamlink account names its sites; two accounts cannot share one.
    let streamlink = |label: &str, site: &str| {
        let mut account = profile("platform-streamlink", label, "SID=1");
        account["sites"] = json!([site]);
        account
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

    let selection = json!(fixed(&ids[0]).to_string());
    set_platform_field(
        &app,
        &token,
        "platform-bilibili",
        "credential_selection",
        selection,
    )
    .await;
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
        .created();
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
    let selection_path =
        |scope: &str| format!("/api/credentials/selection?{scope}&platform_id=platform-bilibili");
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

    let effective = app.get_ok(&selection_path(&streamer_scope), &token).await;
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
    .ok();
    let effective = app.get_ok(&selection_path(&streamer_scope), &token).await;
    assert_eq!(effective["configured"], Value::Null);
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "template", "template_id": template_id})
    );
    assert_eq!(candidate_ids(&effective), [ids[1].clone()]);
    let template_scope = selection_path(&format!("scope_type=template&scope_id={template_id}"));
    assert_eq!(
        app.get_ok(&template_scope, &token).await["configured"],
        fixed(&ids[1])
    );

    app.put(
        &format!("/api/templates/{template_id}"),
        &token,
        &json!({"platform_overrides": {"bilibili": {"credential_selection": {"mode": "inherit"}}}}),
    )
    .await
    .ok();
    let effective = app.get_ok(&selection_path(&streamer_scope), &token).await;
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "platform", "platform_id": "platform-bilibili"})
    );
    assert_eq!(candidate_ids(&effective), [ids[0].clone()]);
    let platform_scope = selection_path("scope_type=platform&scope_id=platform-bilibili");
    assert_eq!(
        app.get_ok(&platform_scope, &token).await["configured"],
        fixed(&ids[0])
    );

    for scope in [
        "scope_type=account&scope_id=x",
        "scope_type=streamer&scope_id=%20",
    ] {
        let refused = app.get(&selection_path(scope), &token).await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    app.delete(&format!("/api/streamers/{streamer_id}"), &token)
        .await
        .ok();
    let removed = app.get(&selection_path(&streamer_scope), &token).await;
    removed.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn account_capabilities_and_checks_answer_without_a_provider_request() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let capabilities = |platform: &str| {
        let path = format!("/api/credentials/capabilities?platform_id={platform}");
        let (app, token) = (&app, &token);
        async move { app.get(&path, token).await }
    };
    assert_eq!(
        capabilities("platform-twitch").await.ok(),
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
    // Streamlink serves many sites, so only its accounts are chosen per streamer.
    let streamlink = capabilities("platform-streamlink").await.ok();
    assert_eq!(streamlink["per_streamer_selection"], true);
    capabilities("%20")
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    capabilities("platform-missing")
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Without an access token or an auth-token cookie there is nothing for
    // Twitch to check, so the check fails before any request.
    let cookie_only = profile("platform-twitch", "Cookie only", "unique_id=abc");
    let unverifiable = create_profile(&app, &token, cookie_only).await;
    let path = format!("/api/credentials/profiles/{}", id_of(&unverifiable));
    let refused = app
        .post(&format!("{path}/validate"), &token, &json!({}))
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(
        app.get_ok(&path, &token).await["health"],
        Value::Null,
        "an unverifiable check records nothing"
    );

    let mut disabled = profile("platform-twitch", "Disabled", "auth-token=abc");
    disabled["enabled"] = json!(false);
    let disabled = create_profile(&app, &token, disabled).await;
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
    let huya = create_profile(&app, &token, profile("platform-huya", "Huya", "udb_uid=1")).await;
    let refreshed = app
        .post(
            &format!("/api/credentials/profiles/{}/refresh", id_of(&huya)),
            &token,
            &json!({}),
        )
        .await
        .ok();
    assert_eq!(refreshed["profile"]["version"], huya["version"]);
    assert_eq!(refreshed["profile"]["revision"], huya["revision"]);
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
    let now = now_ms();
    let target = json!({"type": "create", "platform_id": "platform-bilibili", "label": "QR"});
    let admin = "(SELECT id FROM users WHERE username = 'admin')";
    // Every login expired a moment ago, well inside the receipt window.
    let login = |id: &str, principal: &str, state: &str, result: &str| {
        format!(
            "('{id}', {principal}, 'platform-bilibili', '{target}', 'auth-code', {}, {}, \
             '{state}', {result})",
            now - 120_000,
            now - 1_000
        )
    };
    app.run_sql(&format!(
        "INSERT INTO credential_login_sessions (id, principal, platform_config_id, target, \
             provider_auth_code, created_at, expires_at, state, result_profile_id, \
             result_version, completed_at) VALUES {}, {}, {};",
        login("login-pending", admin, "pending", "NULL, NULL, NULL"),
        login(
            "login-completed",
            admin,
            "completed",
            &format!("'profile-from-qr', 3, {}", now - 2_000)
        ),
        login(
            "login-other-user",
            "'another-user'",
            "pending",
            "NULL, NULL, NULL"
        ),
    ))
    .await;

    let poll = |id: &str| {
        let path = format!("/api/credentials/login-sessions/{id}/poll");
        let (app, token) = (&app, &token);
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
    assert_eq!(poll("login-pending").await.body["status"], "expired");
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

    app.shutdown().await;
}

#[tokio::test]
async fn saved_proxy_login_limits_count_characters() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let proxy = |name: &str, host: u8, username: String, password: String| {
        json!({
            "name": name,
            "url": format!("http://10.20.31.{host}:3128"),
            "username": username,
            "password": password,
        })
    };

    // Three bytes per character: at the limit, but far past it in bytes.
    let at_limit = app
        .post(
            "/api/proxies",
            &token,
            &proxy("At limit", 1, "用".repeat(256), "密".repeat(1024)),
        )
        .await
        .expect_status(StatusCode::CREATED);
    assert_eq!(at_limit.body["username"], "用".repeat(256));

    for (name, host, username, password) in [
        ("Long username", 2, "用".repeat(257), "密".to_owned()),
        ("Long password", 3, "用".to_owned(), "密".repeat(1025)),
    ] {
        app.post(
            "/api/proxies",
            &token,
            &proxy(name, host, username, password),
        )
        .await
        .assert_error(StatusCode::UNPROCESSABLE_ENTITY, "PROXY_INVALID");
    }

    app.shutdown().await;
}
