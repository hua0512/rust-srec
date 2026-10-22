//! Configuration backup export and import over HTTP: what a bundle carries,
//! how an import applies it, and that a rejected import changes nothing.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    ADMIN_PASSWORD, ADMIN_USERNAME, Response, TestApp, WAIT, assert_fields, eventually, id_of,
    now_ms, sorted_keys, tokens_from,
};

async fn export(app: &TestApp, token: &str) -> Value {
    app.get_ok("/api/config/backup/export", token).await
}

async fn import(app: &TestApp, token: &str, config: &Value, mode: &str) -> Response {
    app.post(
        "/api/config/backup/import",
        token,
        &json!({ "config": config, "mode": mode }),
    )
    .await
}

/// A successful import ends every session, the caller's included.
async fn sign_in_again(app: &TestApp, password: &str) -> String {
    app.login_tokens(ADMIN_USERNAME, password).await.access
}

async fn listed_streamers(app: &TestApp, token: &str) -> Vec<Value> {
    app.get_ok("/api/streamers?limit=100", token).await["items"]
        .as_array()
        .expect("streamer items")
        .clone()
}

fn streamer_named<'a>(bundle: &'a mut Value, name: &str) -> &'a mut Value {
    bundle["streamers"]
        .as_array_mut()
        .expect("bundle streamers")
        .iter_mut()
        .find(|streamer| streamer["name"] == name)
        .unwrap_or_else(|| panic!("the bundle has streamer {name}"))
}

/// A bundle without the fields that change on every export or sign-in.
fn comparable(bundle: &Value) -> Value {
    let mut bundle = bundle.clone();
    let object = bundle.as_object_mut().expect("bundle object");
    object.remove("exported_at");
    if let Some(users) = object.get_mut("users").and_then(Value::as_array_mut) {
        for user in users {
            let user = user.as_object_mut().expect("user object");
            for volatile in ["created_at", "updated_at", "last_login_at"] {
                user.remove(volatile);
            }
        }
    }
    // An import stores an absent override document as an empty one; both
    // mean "no overrides".
    for (list, field) in [
        ("streamers", "streamer_specific_config"),
        ("templates", "platform_overrides"),
    ] {
        for entry in object
            .get_mut(list)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if entry[field] == json!({}) {
                entry[field] = Value::Null;
            }
        }
    }
    bundle
}

/// Panics with the JSON paths where two bundles differ, ignoring
/// [`comparable`] noise.
#[track_caller]
fn assert_same_bundle(actual: &Value, expected: &Value) {
    fn walk(left: Option<&Value>, right: Option<&Value>, path: String, out: &mut Vec<String>) {
        match (left, right) {
            (Some(Value::Object(l)), Some(Value::Object(r))) => {
                let mut keys: Vec<&String> = l.keys().chain(r.keys()).collect();
                keys.sort_unstable();
                keys.dedup();
                for key in keys {
                    walk(l.get(key), r.get(key), format!("{path}.{key}"), out);
                }
            }
            (Some(Value::Array(l)), Some(Value::Array(r))) if l.len() == r.len() => {
                for (index, (l, r)) in l.iter().zip(r).enumerate() {
                    walk(Some(l), Some(r), format!("{path}[{index}]"), out);
                }
            }
            _ if left != right => out.push(format!("{path}: {left:?} != {right:?}")),
            _ => {}
        }
    }
    let mut differences = Vec::new();
    walk(
        Some(&comparable(actual)),
        Some(&comparable(expected)),
        String::new(),
        &mut differences,
    );
    assert!(differences.is_empty(), "bundles differ: {differences:#?}");
}

/// Configuration of each kind a bundle carries, created through the API:
/// global settings, a proxy, a Huya account selected for the platform, the
/// template "Backup template", the disabled streamer "Backup Alpha" on it with
/// a time filter, and the webhook "Backup webhook" with subscriptions. Returns
/// the streamer's id.
async fn create_backup_content(app: &TestApp, token: &str) -> String {
    app.patch(
        "/api/config/global",
        token,
        &json!({ "max_concurrent_downloads": 3, "stream_proxy_allow_private_targets": true }),
    )
    .await
    .expect_status(StatusCode::OK);
    app.post(
        "/api/proxies",
        token,
        &json!({
            "name": "Backup proxy",
            "url": "http://127.0.0.1:3128",
            "username": "proxy-user",
            "password": "proxy-secret",
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    let profile = app
        .post(
            "/api/credentials/profiles",
            token,
            &json!({
                "platform_id": "platform-huya",
                "label": "Huya main",
                "enabled": true,
                "material": {
                    "cookies": "session=cookie-secret",
                    "refresh_token": null,
                    "access_token": null,
                    "reauth_config": null,
                },
            }),
        )
        .await
        .expect_status(StatusCode::OK);
    let mut huya = app
        .get_ok("/api/config/platforms/platform-huya", token)
        .await;
    huya["credential_selection"] =
        json!(json!({ "mode": "fixed", "credential_id": id_of(&profile.body) }).to_string());
    app.put("/api/config/platforms/platform-huya", token, &huya)
        .await
        .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            token,
            &json!({ "name": "Backup template" }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    let streamer = id_of(
        &app.create_disabled_streamer(
            token,
            json!({
                "name": "Backup Alpha",
                "url": "https://www.huya.com/990001",
                "template_id": template.body["id"],
                "priority": "HIGH",
            }),
        )
        .await,
    );
    app.post(
        &format!("/api/streamers/{streamer}/filters"),
        token,
        &json!({
            "streamer_id": streamer,
            "filter_type": "TIME_BASED",
            "config": {
                "days_of_week": ["Monday"],
                "start_time": "09:00",
                "end_time": "17:00",
                "timezone": "Europe/Madrid",
            },
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    let channel = app
        .post(
            "/api/notifications/channels",
            token,
            &json!({
                "name": "Backup webhook",
                "channel_type": "Webhook",
                "settings": {
                    "url": "http://127.0.0.1:9/backup",
                    "auth": { "type": "Bearer", "token": "channel-secret" },
                },
            }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    app.put(
        &format!(
            "/api/notifications/channels/{}/subscriptions",
            id_of(&channel.body)
        ),
        token,
        &json!({ "events": ["stream_online", "fatal_error"] }),
    )
    .await
    .expect_status(StatusCode::OK);
    streamer
}

#[tokio::test]
async fn the_export_carries_what_a_restore_needs_and_leaves_out_sessions_and_api_keys() {
    let app = TestApp::start().await;
    let tokens = app.admin().await.clone();
    let token = tokens.access.clone();
    let api_key = app
        .create_api_key(&token, "backup reader", "read_only")
        .await
        .raw;

    let streamer = create_backup_content(&app, &token).await;
    let retired = app
        .post(
            "/api/templates",
            &token,
            &json!({ "name": "Retired template" }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    // No API sets an avatar (the platform's metadata refresh does) or a token
    // in a streamer's own configuration (older installs kept one there). A
    // replace import that dropped a template still in use leaves it retiring.
    app.run_sql(&format!(
        "UPDATE streamers SET avatar = 'https://127.0.0.1/avatar.png', \
             streamer_specific_config = '{{\"refresh_token\":\"streamer-refresh-secret\"}}' \
         WHERE id = '{streamer}';\n\
         INSERT INTO retirement_config_deletions (kind, config_id) VALUES ('template', '{}');",
        id_of(&retired.body),
    ))
    .await;

    let response = app
        .get("/api/config/backup/export", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(response.header("content-type"), Some("application/json"));
    let disposition = response.header("content-disposition").unwrap_or_default();
    assert!(
        disposition.starts_with("attachment; filename=\"rust-srec-backup-")
            && disposition.ends_with(".json\""),
        "{disposition}"
    );
    let text = &response.text;
    let bundle: Value = serde_json::from_str(text).expect("export JSON");

    let mut top_level: Vec<&str> = sorted_keys(&bundle);
    top_level.retain(|key| !matches!(*key, "job_presets" | "pipeline_presets"));
    assert_eq!(
        top_level,
        [
            "credential_profiles",
            "engines",
            "exported_at",
            "global_config",
            "notification_channels",
            "platforms",
            "proxies",
            "streamers",
            "templates",
            "users",
            "version"
        ]
    );
    assert_eq!(
        bundle["version"], "1.0.0",
        "accounts and proxies need the managed format"
    );

    let users = bundle["users"].as_array().expect("users");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["username"], ADMIN_USERNAME);
    assert_eq!(users[0]["is_active"], true);
    assert_eq!(users[0]["must_change_password"], false);
    let hash = users[0]["password_hash"].as_str().expect("password hash");
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    let roles = users[0]["roles"].as_array().expect("roles");
    assert!(roles.contains(&json!("admin")), "{roles:?}");

    let profiles = bundle["credential_profiles"].as_array().expect("profiles");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0]["platform"], "huya");
    assert_eq!(profiles[0]["label"], "Huya main");
    assert_eq!(profiles[0]["cookies"], "session=cookie-secret");
    assert_eq!(
        bundle["proxies"],
        json!([{
            "name": "Backup proxy",
            "url": "http://127.0.0.1:3128",
            "username": "proxy-user",
            "password": "proxy-secret",
        }])
    );
    assert_eq!(
        bundle["notification_channels"],
        json!([{
            "name": "Backup webhook",
            "channel_type": "Webhook",
            "settings": {
                "url": "http://127.0.0.1:9/backup",
                "auth": { "type": "Bearer", "token": "channel-secret" },
            },
            "subscriptions": ["fatal_error", "stream_online"],
        }]),
        "channel settings are an object, not a string"
    );

    let streamers = bundle["streamers"].as_array().expect("streamers");
    assert_eq!(streamers.len(), 1);
    assert_fields(
        &streamers[0],
        json!({
            "name": "Backup Alpha", "url": "https://www.huya.com/990001", "platform": "huya",
            "state": "DISABLED", "avatar_url": "https://127.0.0.1/avatar.png",
        }),
    );
    assert_eq!(
        streamers[0]["streamer_specific_config"]["refresh_token"],
        "streamer-refresh-secret"
    );

    let global = &bundle["global_config"];
    let stored = app.get_ok("/api/config/global", &token).await;
    let log_filter = global["log_filter_directive"].as_str().expect("log filter");
    assert_eq!(log_filter, stored["log_filter_directive"]);
    assert!(!log_filter.is_empty());
    assert_eq!(global["stream_proxy_allow_private_targets"], true);
    assert!(global.get("proxy_route").is_some());
    assert!(
        global.get("proxy_config").is_none(),
        "no legacy proxy_config"
    );

    let templates = bundle["templates"].as_array().expect("templates");
    let template_names: Vec<&Value> = templates.iter().map(|template| &template["name"]).collect();
    assert!(template_names.contains(&&json!("Backup template")));
    assert!(
        !template_names.contains(&&json!("Retired template")),
        "a retiring template is left out"
    );
    for scope in templates
        .iter()
        .chain(bundle["platforms"].as_array().expect("platforms"))
    {
        assert!(scope.get("cookies").is_none(), "{scope}");
        assert!(scope.get("proxy_config").is_none(), "{scope}");
    }

    for secret in [&tokens.access, &tokens.refresh, &api_key] {
        assert!(
            !text.contains(secret.as_str()),
            "the export carries a session secret"
        );
    }
    for field in [
        "refresh_token_hash",
        "token_hash",
        "api_keys",
        "auth_sessions",
        "key_hash",
    ] {
        assert!(!text.contains(field), "the export mentions {field}");
    }

    // The bundle holds every secret, so a read-only key cannot fetch or restore one.
    for (method, path, body) in [
        (Method::GET, "/api/config/backup/export", None),
        (
            Method::POST,
            "/api/config/backup/import",
            Some(json!({ "config": bundle, "mode": "merge" })),
        ),
    ] {
        let response = app.send(method, path, Some(&api_key), body.as_ref()).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(response.error_code(), Some("API_KEY_READ_ONLY"));
    }

    app.shutdown().await;
}

#[tokio::test]
async fn a_backup_restores_into_a_fresh_install_and_exports_the_same_bundle() {
    const RESTORED_PASSWORD: &str = "e2e-restored-Password-2";
    let source = TestApp::start().await;
    let token = source.admin_token().await;
    // A password the fresh install never had, so signing in there proves the
    // hash travelled with the bundle.
    source
        .post(
            "/api/auth/change-password",
            &token,
            &json!({ "current_password": ADMIN_PASSWORD, "new_password": RESTORED_PASSWORD }),
        )
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&source, RESTORED_PASSWORD).await;

    create_backup_content(&source, &token).await;
    let bundle = export(&source, &token).await;
    source.shutdown().await;

    let target = TestApp::start().await;
    let target_token = target.admin_token().await;
    let result = import(&target, &target_token, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(result.body["success"], true);
    assert!(result.body["message"].is_string());
    let stats = &result.body["stats"];
    let mut counters = Vec::new();
    for entity in [
        "templates",
        "streamers",
        "engines",
        "channels",
        "job_presets",
        "pipeline_presets",
        "users",
    ] {
        for change in ["created", "updated", "deleted"] {
            counters.push(format!("{entity}_{change}"));
        }
    }
    counters.push("platforms_updated".to_owned());
    counters.sort_unstable();
    assert_eq!(
        sorted_keys(stats),
        counters,
        "the frontend renders every counter"
    );
    for counter in &counters {
        assert!(stats[counter].is_u64(), "{counter}: {stats}");
    }
    // The admin is matched by username.
    assert_fields(
        stats,
        json!({
            "templates_created": 1, "streamers_created": 1, "channels_created": 1,
            "users_created": 0, "users_updated": 1,
            "platforms_updated": bundle["platforms"].as_array().expect("platforms").len(),
        }),
    );
    for change in counters
        .iter()
        .filter(|counter| counter.ends_with("_deleted"))
    {
        assert_eq!(stats[change], 0, "a merge deletes nothing: {change}");
    }

    let stale = target.get("/api/streamers", &target_token).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let wrong = target.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    assert_eq!(
        wrong.status,
        StatusCode::UNAUTHORIZED,
        "the old password is gone"
    );
    let token = sign_in_again(&target, RESTORED_PASSWORD).await;

    let streamers = listed_streamers(&target, &token).await;
    assert_eq!(streamers.len(), 1);
    let restored = &streamers[0];
    assert_fields(
        restored,
        json!({
            "name": "Backup Alpha", "url": "https://www.huya.com/990001", "state": "DISABLED",
            "priority": "HIGH",
        }),
    );
    let template_id = restored["template_id"].as_str().expect("template assigned");
    let template = target
        .get_ok(&format!("/api/templates/{template_id}"), &token)
        .await;
    assert_eq!(template["name"], "Backup template");
    let instances = target.get_ok("/api/notifications/instances", &token).await;
    assert!(
        instances
            .as_array()
            .expect("instances")
            .iter()
            .any(|instance| instance["display_name"] == "Backup webhook"),
        "the restored channel is loaded: {instances}"
    );
    let global = target.get_ok("/api/config/global", &token).await;
    assert_eq!(global["max_concurrent_downloads"], 3);
    assert_eq!(global["stream_proxy_allow_private_targets"], true);

    let again = export(&target, &token).await;
    assert_same_bundle(&again, &bundle);

    target.shutdown().await;
}

const SEEDED_LOG_FILTER: &str = "rust_srec=info,sqlx=warn";
const SEEDED_AVATAR: &str = "https://127.0.0.1/seeded-avatar.png";

#[tokio::test]
async fn an_import_resets_omitted_flags_keeps_the_log_filter_and_clears_omitted_avatars() {
    let now = now_ms();
    let app = TestApp::start_seeded(&format!(
        "UPDATE global_config SET log_filter_directive = '{SEEDED_LOG_FILTER}', stream_proxy_allow_private_targets = 1;
         INSERT INTO streamers (id, name, url, platform_config_id, state, priority, avatar, created_at, updated_at)
         VALUES ('seeded-streamer', 'Seeded', 'https://www.huya.com/550001', 'platform-huya', 'DISABLED', 'NORMAL', '{SEEDED_AVATAR}', {now}, {now});"
    ))
    .await;
    let token = app.admin_token().await;
    let streamer = async |token: &str| app.get_ok("/api/streamers/seeded-streamer", token).await;
    assert_eq!(streamer(&token).await["avatar_url"], SEEDED_AVATAR);

    let mut bundle = export(&app, &token).await;
    assert_fields(
        &bundle["global_config"],
        json!({
            "stream_proxy_allow_private_targets": true, "log_filter_directive": SEEDED_LOG_FILTER,
        }),
    );
    let invalid_mode = import(&app, &token, &bundle, "overwrite").await;
    assert_eq!(invalid_mode.status, StatusCode::UNPROCESSABLE_ENTITY);

    let global = bundle["global_config"]
        .as_object_mut()
        .expect("global config");
    global.remove("stream_proxy_allow_private_targets");
    global.remove("log_filter_directive");
    streamer_named(&mut bundle, "Seeded")
        .as_object_mut()
        .expect("streamer")
        .remove("avatar_url");
    import(&app, &token, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;

    let global = app.get_ok("/api/config/global", &token).await;
    assert_eq!(
        global["stream_proxy_allow_private_targets"], false,
        "an omitted private-target flag fails closed"
    );
    assert_eq!(global["log_filter_directive"], SEEDED_LOG_FILTER);
    assert_eq!(streamer(&token).await["avatar_url"], Value::Null);
    let cleared = "SELECT COUNT(*) FROM streamers WHERE id = 'seeded-streamer' AND avatar IS NULL";
    assert_eq!(app.query_i64(cleared).await, 1);

    app.shutdown().await;
}

#[tokio::test]
async fn omitted_time_filter_zones_mean_local_before_schema_0_1_8_and_utc_from_it() {
    let app = TestApp::start().await;
    let mut token = app.admin_token().await;
    app.disabled_streamer(&token, "Zoned", "https://www.huya.com/440001")
        .await;
    let unzoned = json!([{
        "filter_type": "TIME_BASED",
        "config": { "days_of_week": ["Monday"], "start_time": "09:00", "end_time": "17:00" },
    }]);

    for (version, expected) in [("0.1.7", "local"), ("0.1.8", "UTC")] {
        let mut bundle = export(&app, &token).await;
        assert_eq!(bundle["version"], "0.1.8");
        bundle["version"] = json!(version);
        streamer_named(&mut bundle, "Zoned")["filters"] = unzoned.clone();
        import(&app, &token, &bundle, "merge")
            .await
            .expect_status(StatusCode::OK);
        token = sign_in_again(&app, ADMIN_PASSWORD).await;

        let mut exported = export(&app, &token).await;
        let filters = streamer_named(&mut exported, "Zoned")["filters"].clone();
        assert_eq!(filters.as_array().map(Vec::len), Some(1), "{version}");
        assert_eq!(
            filters[0]["config"]["timezone"], expected,
            "version {version}"
        );
    }

    app.shutdown().await;
}

#[tokio::test]
async fn merge_keeps_and_replace_removes_omitted_streamers_and_a_case_only_url_updates_in_place() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let kept = app
        .disabled_streamer(&token, "Case Room", "https://www.huya.com/caseroom")
        .await;
    let omitted = app
        .disabled_streamer(&token, "Omitted", "https://www.huya.com/330002")
        .await;

    let mut bundle = export(&app, &token).await;
    bundle["streamers"]
        .as_array_mut()
        .expect("streamers")
        .retain(|streamer| streamer["name"] != "Omitted");
    streamer_named(&mut bundle, "Case Room")["url"] = json!("https://www.huya.com/CaseRoom");

    let merged = import(&app, &token, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(merged.body["stats"]["streamers_created"], 0);
    assert_eq!(merged.body["stats"]["streamers_updated"], 1);
    assert_eq!(merged.body["stats"]["streamers_deleted"], 0);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    // The merge keeps the omitted streamer and updates the case-only match in
    // place.
    let mut urls: Vec<(String, Value)> = listed_streamers(&app, &token)
        .await
        .into_iter()
        .map(|streamer| (id_of(&streamer), streamer["url"].clone()))
        .collect();
    urls.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![
        (kept.clone(), json!("https://www.huya.com/CaseRoom")),
        (omitted, json!("https://www.huya.com/330002")),
    ];
    expected.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(urls, expected);

    let mut no_admin = bundle.clone();
    for user in no_admin["users"].as_array_mut().expect("users") {
        user["is_active"] = json!(false);
    }
    let rejected = import(&app, &token, &no_admin, "replace")
        .await
        .expect_status(StatusCode::BAD_REQUEST);
    assert_eq!(rejected.error_code(), Some("BAD_REQUEST"));
    let message = rejected.body["message"].as_str().expect("message");
    assert!(message.contains("'admin' role"), "{message}");
    assert_eq!(listed_streamers(&app, &token).await.len(), 2);

    let replaced = import(&app, &token, &bundle, "replace")
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(replaced.body["stats"]["streamers_updated"], 1);
    assert_eq!(replaced.body["stats"]["streamers_deleted"], 1);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    let remaining = eventually(WAIT, "the omitted streamer to be removed", || async {
        let streamers = listed_streamers(&app, &token).await;
        (streamers.len() == 1).then_some(streamers)
    })
    .await;
    assert_eq!(remaining[0]["id"], kept.as_str());

    app.shutdown().await;
}

/// A rejected import changes nothing and ends no session; a successful one
/// then ends every session.
#[tokio::test]
async fn a_rejected_import_changes_nothing_and_a_successful_one_ends_every_session() {
    let app = TestApp::start().await;
    let first = app.admin().await.clone();
    let token = first.access.clone();
    app.disabled_streamer(&token, "Stable", "https://www.huya.com/220001")
        .await;
    // A row still being removed keeps its URL; no API leaves one behind.
    // Inserted after start, the streamer manager never loads it, so the
    // reaper leaves it in place.
    let retiring_url = "https://www.huya.com/220099";
    let now = now_ms();
    app.run_sql(&format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, priority, created_at, \
             updated_at, deleted_at) \
         VALUES ('retiring-streamer', 'Retiring', '{retiring_url}', 'platform-huya', 'DISABLED', \
             'NORMAL', {now}, {now}, {now});"
    ))
    .await;

    let baseline = export(&app, &token).await;
    let new_streamer = |name: &str, url: &str| {
        json!({
            "name": name,
            "url": url,
            "platform": "huya",
            "template": null,
            "priority": "NORMAL",
            "state": "DISABLED",
            "avatar_url": null,
            "streamer_specific_config": null,
            "filters": [],
        })
    };
    type Corrupt = Box<dyn Fn(&mut Value)>;
    let cases: [(&str, Corrupt, &str); 8] = [
        (
            "unknown platform config",
            Box::new(|bundle| {
                bundle["platforms"]
                    .as_array_mut()
                    .expect("platforms")
                    .push(json!({ "platform_name": "noplatformhere" }))
            }),
            "noplatformhere",
        ),
        (
            "unknown platform",
            Box::new(|bundle| {
                streamer_named(bundle, "Stable")["platform"] = json!("nosuchplatform")
            }),
            "nosuchplatform",
        ),
        (
            "unknown template",
            Box::new(|bundle| {
                streamer_named(bundle, "Stable")["template"] = json!("Missing template")
            }),
            "Missing template",
        ),
        (
            "duplicate URLs",
            Box::new(move |bundle| {
                bundle["streamers"]
                    .as_array_mut()
                    .expect("streamers")
                    .push(new_streamer("Shadow", "https://www.HUYA.com/220001"))
            }),
            "https://www.HUYA.com/220001",
        ),
        (
            "invalid channel settings",
            Box::new(|bundle| {
                bundle["notification_channels"]
                    .as_array_mut()
                    .expect("channels")
                    .push(json!({
                        "name": "Broken",
                        "channel_type": "Webhook",
                        "settings": { "method": "POST" },
                        "subscriptions": [],
                    }))
            }),
            "'Broken'",
        ),
        (
            "invalid filter timezone",
            Box::new(|bundle| {
                streamer_named(bundle, "Stable")["filters"] = json!([{
                    "filter_type": "TIME_BASED",
                    "config": {
                        "days_of_week": ["Monday"],
                        "start_time": "09:00",
                        "end_time": "17:00",
                        "timezone": "Mars/Olympus_Mons",
                    },
                }])
            }),
            "Mars/Olympus_Mons",
        ),
        (
            "retiring URL",
            Box::new(move |bundle| {
                bundle["streamers"]
                    .as_array_mut()
                    .expect("streamers")
                    .push(new_streamer("Returning", retiring_url))
            }),
            "still being removed",
        ),
        (
            "unsupported version",
            Box::new(|bundle| bundle["version"] = json!("2.0.0")),
            "2.0.0",
        ),
    ];
    for (case, corrupt, message) in cases {
        let mut bundle = baseline.clone();
        // A valid change riding along must not be applied either.
        bundle["global_config"]["max_concurrent_downloads"] = json!(9);
        corrupt(&mut bundle);
        let response = import(&app, &token, &bundle, "merge").await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{case}: {}",
            response.text
        );
        assert_eq!(response.error_code(), Some("BAD_REQUEST"), "{case}");
        let text = response.body["message"].as_str().expect("message");
        assert!(text.contains(message), "{case}: {text}");
    }

    // The tokens still work, so nothing was revoked, and nothing was written.
    let after = export(&app, &token).await;
    assert_same_bundle(&after, &baseline);
    let streamers = listed_streamers(&app, &token).await;
    assert_eq!(streamers.len(), 1);
    assert_eq!(streamers[0]["name"], "Stable");
    let refreshed = app
        .refresh(&first.refresh)
        .await
        .expect_status(StatusCode::OK);
    let second = tokens_from(&refreshed.body);

    // Revocation does not depend on the bundle replacing any user.
    let mut bundle = baseline;
    bundle.as_object_mut().expect("bundle").remove("users");
    import(&app, &second.access, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    for response in [
        app.get("/api/streamers", &first.access).await,
        app.get("/api/streamers", &second.access).await,
        app.refresh(&second.refresh).await,
    ] {
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "{}",
            response.text
        );
    }
    // Signing in still works: the import revoked sessions, not the account.
    sign_in_again(&app, ADMIN_PASSWORD).await;

    app.shutdown().await;
}

#[tokio::test]
async fn a_late_database_failure_rolls_back_a_replace_import() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    app.disabled_streamer(&token, "Kept", "https://www.huya.com/110001")
        .await;
    let dropped = app
        .disabled_streamer(&token, "Dropped", "https://www.huya.com/110002")
        .await;
    let baseline = export(&app, &token).await;

    // Fail the channel write, but only once the import has marked the dropped
    // streamer for removal: if channels were ever written first, the import
    // would succeed and this test would fail rather than pass with nothing to
    // roll back.
    app.run_sql(&format!(
        "CREATE TRIGGER e2e_fail_channel_import BEFORE INSERT ON notification_channel
         WHEN (SELECT deleted_at FROM streamers WHERE id = '{dropped}') IS NOT NULL
         BEGIN SELECT RAISE(ABORT, 'injected import failure'); END"
    ))
    .await;

    let mut bundle = baseline.clone();
    bundle["global_config"]["max_concurrent_downloads"] = json!(9);
    bundle["streamers"]
        .as_array_mut()
        .expect("streamers")
        .retain(|streamer| streamer["name"] != "Dropped");
    bundle["notification_channels"] = json!([{
        "name": "Late",
        "channel_type": "Discord",
        "settings": { "webhook_url": "http://127.0.0.1:9/late" },
        "subscriptions": [],
    }]);
    let response = import(&app, &token, &bundle, "replace")
        .await
        .expect_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.error_code(), Some("INTERNAL_ERROR"));
    assert!(!response.text.contains("injected"), "{}", response.text);

    let unmarked =
        format!("SELECT COUNT(*) FROM streamers WHERE id = '{dropped}' AND deleted_at IS NULL");
    assert_eq!(
        app.query_i64(&unmarked).await,
        1,
        "the removal mark was rolled back"
    );
    let streamers = listed_streamers(&app, &token).await;
    let mut names: Vec<&str> = streamers
        .iter()
        .map(|streamer| streamer["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["Dropped", "Kept"]);
    assert!(
        streamers
            .iter()
            .all(|streamer| streamer["state"] == "DISABLED")
    );
    // The original token still works: the failed import revoked nothing.
    let after = export(&app, &token).await;
    assert_same_bundle(&after, &baseline);

    app.shutdown().await;
}
