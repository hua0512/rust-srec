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

/// The global, Huya and template settings as the settings pages read them,
/// without the IDs and times that stay local to an install.
async fn settings_shown(app: &TestApp, token: &str, template_id: &str) -> Value {
    let mut global = app.get_ok("/api/config/global", token).await;
    let mut huya = app
        .get_ok("/api/config/platforms/platform-huya", token)
        .await;
    let mut template = app
        .get_ok(&format!("/api/templates/{template_id}"), token)
        .await;
    for (view, local) in [
        (&mut global, &["proxy_route"][..]),
        (&mut huya, &["credential_selection"][..]),
        (&mut template, &["id", "created_at", "updated_at"][..]),
    ] {
        let view = view.as_object_mut().expect("settings object");
        for field in local {
            view.remove(*field);
        }
    }
    // As in `comparable`, empty overrides and none mean the same.
    if template["platform_overrides"] == json!({}) {
        template["platform_overrides"] = Value::Null;
    }
    json!({ "global": global, "huya": huya, "template": template })
}

/// Configuration of each kind a bundle carries, created through the API:
/// global settings, a proxy, a Huya account selected for the platform, the
/// template "Backup template", the disabled streamer "Backup Alpha" on it with
/// a time filter, the webhook "Backup webhook" with subscriptions, delivering
/// through the proxy, and the rclone job preset "Backup upload" uploading
/// through it. Returns
/// the streamer's id.
///
/// The global, Huya and template settings differ from a fresh install's, so a
/// restore that drops one of them shows.
async fn create_backup_content(app: &TestApp, token: &str) -> String {
    app.patch(
        "/api/config/global",
        token,
        &json!({
            "max_concurrent_downloads": 3,
            "stream_proxy_allow_private_targets": true,
            "default_extractor": "streamlink",
            "default_download_engine": "default-ffmpeg",
            "record_danmu": true,
            "offline_check_count": 7,
            "output_retention_days": 14,
            "auto_thumbnail": false,
        }),
    )
    .await
    .expect_status(StatusCode::OK);
    let proxy = app
        .post(
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
    app.patch(
        "/api/config/global",
        token,
        &json!({"proxy_route": {"kind": "proxy", "id": proxy.body["id"]}}),
    )
    .await
    .ok();
    let upload = json!({"destination_root": "remote:/backup", "proxy_route": {"kind": "proxy", "id": proxy.body["id"]}});
    app.post(
        "/api/job/presets",
        token,
        &json!({
            "id": "backup-upload", "name": "Backup upload", "category": "upload",
            "processor": "rclone", "config": upload.to_string(),
        }),
    )
    .await
    .created();
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
    for (field, value) in [
        ("extractor", json!("streamlink")),
        ("download_engine", json!("default-ffmpeg")),
        ("fetch_delay_ms", json!(1234)),
        ("record_danmu", json!(true)),
        ("output_folder", json!("/srv/huya")),
        ("offline_check_count", json!(4)),
    ] {
        huya[field] = value;
    }
    app.put("/api/config/platforms/platform-huya", token, &huya)
        .await
        .expect_status(StatusCode::OK);
    let template = app
        .post(
            "/api/templates",
            token,
            &json!({
                "name": "Backup template",
                "extractor": "streamlink",
                "download_engine": "default-ffmpeg",
                "output_folder": "/srv/template",
                "record_danmu": false,
                "min_segment_size_bytes": 2048,
                "offline_check_count": 5,
            }),
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
                "proxy_route": {"kind": "proxy", "id": proxy.body["id"]},
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
    assert_eq!(response.header("cache-control"), Some("private, no-store"));
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
            "proxy_route": {"kind": "proxy", "name": "Backup proxy"},
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

    let upload = bundle["job_presets"]
        .as_array()
        .expect("job presets")
        .iter()
        .find(|preset| preset["name"] == "Backup upload")
        .expect("the upload preset is exported");
    assert_eq!(
        upload["config"]["proxy_route"],
        json!({"kind": "proxy", "name": "Backup proxy"}),
        "upload steps name their proxy"
    );

    let global = &bundle["global_config"];
    let stored = app.get_ok("/api/config/global", &token).await;
    let log_filter = global["log_filter_directive"].as_str().expect("log filter");
    assert_eq!(log_filter, stored["log_filter_directive"]);
    assert!(!log_filter.is_empty());
    assert_eq!(global["stream_proxy_allow_private_targets"], true);
    assert_eq!(
        global["proxy_route"],
        json!({"kind": "proxy", "name": "Backup proxy"})
    );
    let proxy_id = app.get_ok("/api/proxies", &token).await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        !text.contains(&proxy_id),
        "proxy ids stay local to the install"
    );
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

    let streamer = create_backup_content(&source, &token).await;
    let template_id = source
        .get_ok(&format!("/api/streamers/{streamer}"), &token)
        .await["template_id"]
        .as_str()
        .expect("template assigned")
        .to_owned();
    let settings_before = settings_shown(&source, &token, &template_id).await;
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
    assert_eq!(
        settings_shown(&target, &token, template_id).await,
        settings_before
    );
    // Upload steps name their proxy by name in the bundle and by the
    // installation's own ID once restored.
    let proxy_path = format!(
        "/api/proxies/{}",
        target.get_ok("/api/proxies", &token).await[0]["id"]
            .as_str()
            .expect("restored proxy")
    );
    let references = |detail: Value| detail["references"].clone();
    assert_eq!(
        references(target.get_ok(&proxy_path, &token).await)["job_presets"][0]["name"],
        "Backup upload"
    );

    let again = export(&target, &token).await;
    assert_same_bundle(&again, &bundle);

    let mut bundle = again;
    // A definition encoded as a JSON string is read like one that is not.
    bundle["pipeline_presets"] = json!([{
        "name": "Imported workflow", "description": null, "pipeline_type": null,
        "dag_definition": json!({"name": "imported", "steps": [
            {"id": "run", "step": {"type": "inline", "processor": "execute", "config": {"command": "exit 0"}}},
            {"id": "ship", "step": {"type": "inline", "processor": "rclone", "config": {
                "destination_root": "remote:/", "proxy_route": {"kind": "proxy", "name": "Backup proxy"},
            }}, "depends_on": ["run"]},
        ]}).to_string(),
    }]);
    let created = import(&target, &token, &bundle, "merge").await.ok();
    assert_eq!(created["stats"]["pipeline_presets_created"], 1);
    let token = sign_in_again(&target, RESTORED_PASSWORD).await;
    assert_eq!(
        references(target.get_ok(&proxy_path, &token).await)["pipelines"][0]["name"],
        "Imported workflow"
    );
    let mut bundle = export(&target, &token).await;
    bundle["pipeline_presets"]
        .as_array_mut()
        .unwrap()
        .retain(|preset| preset["name"] == "Imported workflow");
    assert_eq!(bundle["pipeline_presets"].as_array().unwrap().len(), 1);
    assert_eq!(bundle["pipeline_presets"][0]["pipeline_type"], "dag");
    // Updating a backup keeps extension fields the typed DAG does not model.
    bundle["pipeline_presets"][0]["dag_definition"]["x_layout"] = json!({"run": [10, 20]});
    bundle["pipeline_presets"][0]["description"] = json!("updated");
    bundle["pipeline_presets"][0]["pipeline_type"] = json!("legacy");
    let updated = import(&target, &token, &bundle, "merge").await.ok();
    assert_eq!(updated["stats"]["pipeline_presets_updated"], 1);
    let token = sign_in_again(&target, RESTORED_PASSWORD).await;
    let exported = export(&target, &token).await;
    assert_eq!(
        exported["pipeline_presets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|preset| preset["name"] == "Imported workflow")
            .unwrap(),
        &bundle["pipeline_presets"][0]
    );

    target.shutdown().await;
}

#[tokio::test]
async fn a_replace_import_keeps_an_omitted_preset_a_kept_pipeline_names() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    app.post(
        "/api/job/presets",
        &token,
        &json!({
            "id": "e2e-archive", "name": "E2E Archive", "category": "file_ops",
            "processor": "copy_move",
            "config": "{\"destination\":\"/archive\",\"operation\":\"copy\"}",
        }),
    )
    .await
    .created();
    for (name, step) in [
        (
            "E2E Tidy",
            json!({"type": "inline", "processor": "delete", "config": {}}),
        ),
        ("E2E Chain", json!({"type": "workflow", "name": "E2E Tidy"})),
    ] {
        app.post(
            "/api/pipeline/presets",
            &token,
            &json!({"name": name, "dag": {"name": name, "steps": [{"id": "only", "step": step}]}}),
        )
        .await
        .created();
    }
    let pipeline = json!({"name": "Evening", "steps": [
        {"id": "archive", "step": {"type": "preset", "name": "E2E Archive"}},
    ]});
    app.post(
        "/api/templates",
        &token,
        &json!({"name": "Evening", "pipeline": pipeline.to_string()}),
    )
    .await
    .created();
    let omitted_account = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-bilibili",
                "label": "Omitted Bilibili",
                "enabled": true,
                "material": { "cookies": "SESSDATA=omitted-secret" },
            }),
        )
        .await
        .expect_status(StatusCode::OK);
    let mut bilibili = app
        .get_ok("/api/config/platforms/platform-bilibili", &token)
        .await;
    bilibili["download_engine"] = json!("default-ffmpeg");
    bilibili["output_folder"] = json!("/srv/bilibili");
    bilibili["credential_selection"] = json!(
        json!({ "mode": "fixed", "credential_id": id_of(&omitted_account.body) }).to_string()
    );
    app.put("/api/config/platforms/platform-bilibili", &token, &bilibili)
        .await
        .expect_status(StatusCode::OK);

    // Omit the preset the kept template names, a workflow chain nothing kept
    // names, and the customized Bilibili platform and its account.
    let mut bundle = export(&app, &token).await;
    let omitted = ["E2E Archive", "E2E Tidy", "E2E Chain"];
    for list in ["job_presets", "pipeline_presets"] {
        bundle[list]
            .as_array_mut()
            .expect("bundle presets")
            .retain(|preset| !omitted.iter().any(|name| preset["name"] == *name));
    }
    bundle["platforms"]
        .as_array_mut()
        .expect("bundle platforms")
        .retain(|platform| {
            platform["platform_name"] != "bilibili" && platform["platform_name"] != "streamlink"
        });
    bundle["credential_profiles"]
        .as_array_mut()
        .expect("bundle profiles")
        .retain(|profile| profile["label"] != "Omitted Bilibili");
    let replaced = import(&app, &token, &bundle, "replace").await.ok();
    assert_eq!(replaced["stats"]["job_presets_deleted"], 0);
    assert_eq!(replaced["stats"]["pipeline_presets_deleted"], 2);
    let message = replaced["message"].as_str().expect("message");
    assert!(
        message.contains("kept preset 'E2E Archive'") && message.contains("template 'Evening'"),
        "{message}"
    );
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    let bilibili_after = app
        .get_ok("/api/config/platforms/platform-bilibili", &token)
        .await;
    assert_eq!(bilibili_after["download_engine"], Value::Null);
    assert_eq!(bilibili_after["output_folder"], Value::Null);
    assert_eq!(bilibili_after["credential_selection"], Value::Null);
    let streamlink_after = app
        .get_ok("/api/config/platforms/platform-streamlink", &token)
        .await;
    assert_eq!(streamlink_after["download_engine"], "default-streamlink");
    assert_eq!(
        streamlink_after["platform_specific_config"],
        "{\"streamlink\":{}}"
    );
    assert_eq!(
        app.get_ok(
            "/api/credentials/profiles?platform_id=platform-bilibili",
            &token
        )
        .await,
        json!([])
    );

    app.shutdown().await;
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
    // Fields the app does not know survive the zone normalisation.
    let unzoned = json!([{
        "filter_type": "TIME_BASED",
        "config": {
            "days_of_week": ["Monday"],
            "start_time": "09:00",
            "end_time": "17:00",
            "extension": { "kept": true },
        },
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
        assert_eq!(
            filters[0]["config"]["extension"],
            json!({ "kept": true }),
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

    // A replace must keep an active admin: an inactive admin and an active
    // user without the role both fail.
    for (is_active, role) in [(false, "admin"), (true, "user")] {
        let mut no_admin = bundle.clone();
        for user in no_admin["users"].as_array_mut().expect("users") {
            user["is_active"] = json!(is_active);
            user["roles"] = json!([role]);
        }
        let rejected = import(&app, &token, &no_admin, "replace")
            .await
            .expect_status(StatusCode::BAD_REQUEST);
        assert_eq!(rejected.error_code(), Some("BAD_REQUEST"));
        let message = rejected.body["message"].as_str().expect("message");
        assert!(message.contains("'admin' role"), "{role}: {message}");
        assert_eq!(listed_streamers(&app, &token).await.len(), 2);
    }

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

/// Account selections on the platform, a template and a streamer travel with
/// the bundle: the export spells out a pool's defaults, a replace restores
/// them exactly, a merge keeps what the bundle omits, and a bundle from before
/// accounts existed clears every selection but keeps the accounts.
#[tokio::test]
async fn backups_carry_account_selections_through_replace_and_merge_imports() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let streamer = app
        .disabled_streamer(&token, "Selecting", "https://www.huya.com/440077")
        .await;
    let streamer_path = format!("/api/streamers/{streamer}");

    // Without accounts or selections the bundle keeps the shape older
    // releases read.
    let plain = export(&app, &token).await;
    assert_eq!(plain["version"], "0.1.8");
    assert!(plain.get("credential_profiles").is_none(), "{plain}");
    assert!(
        plain["platforms"]
            .as_array()
            .expect("platforms")
            .iter()
            .all(|platform| platform.get("credential_selection").is_none()),
        "{plain}"
    );
    // A selection alone, even `none` with no accounts, needs schema 1.0.0, or
    // the bundle would be refused by its own import.
    app.put(
        &streamer_path,
        &token,
        &json!({"streamer_specific_config": {"credential_selection": {"mode": "none"}}}),
    )
    .await
    .ok();
    let none_only = export(&app, &token).await;
    assert_eq!(none_only["version"], "1.0.0");
    import(&app, &token, &none_only, "merge")
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;

    let mut accounts = Vec::new();
    for label in ["Huya a", "Huya b"] {
        let created = app
            .post(
                "/api/credentials/profiles",
                &token,
                &json!({
                    "platform_id": "platform-huya",
                    "label": label,
                    "enabled": true,
                    "material": { "cookies": format!("session={label}") },
                }),
            )
            .await
            .expect_status(StatusCode::OK);
        accounts.push(id_of(&created.body));
    }
    let (a, b) = (accounts[0].as_str(), accounts[1].as_str());
    let fixed = |id: &str| json!({ "mode": "fixed", "credential_id": id });
    let set_platform_selection = |selection: Value| {
        let app = &app;
        let token = &token;
        async move {
            let mut huya = app
                .get_ok("/api/config/platforms/platform-huya", token)
                .await;
            huya["credential_selection"] = json!(selection.to_string());
            app.put("/api/config/platforms/platform-huya", token, &huya)
                .await
                .expect_status(StatusCode::OK);
        }
    };
    set_platform_selection(fixed(a)).await;
    let template = app
        .post(
            "/api/templates",
            &token,
            &json!({
                "name": "Selecting",
                "platform_overrides": {"huya": {
                    "credential_selection": {"mode": "pool", "credential_ids": [b, a]},
                    "quality": 1,
                }},
            }),
        )
        .await
        .created();
    app.put(
        &streamer_path,
        &token,
        &json!({
            "template_id": id_of(&template),
            "streamer_specific_config": {"credential_selection": {"mode": "none"}},
        }),
    )
    .await
    .ok();

    // The export moves to schema 1.0.0 and spells out the pool's defaults.
    let selected = export(&app, &token).await;
    assert_eq!(selected["version"], "1.0.0");
    assert_eq!(
        selected["credential_profiles"].as_array().map(Vec::len),
        Some(2)
    );
    let huya_selection = |bundle: &Value| {
        bundle["platforms"]
            .as_array()
            .expect("platforms")
            .iter()
            .find(|platform| platform["platform_name"] == "huya")
            .expect("the huya platform")["credential_selection"]
            .clone()
    };
    let template_overrides = |bundle: &Value| {
        bundle["templates"]
            .as_array()
            .expect("templates")
            .iter()
            .find(|template| template["name"] == "Selecting")
            .expect("the selecting template")["platform_overrides"]
            .clone()
    };
    let spelled_pool = |quality: i64| {
        json!({"huya": {
            "credential_selection": {
                "mode": "pool",
                "credential_ids": [b, a],
                "strategy": "priority",
                "failover": true,
                "max_attempts": 3,
            },
            "quality": quality,
        }})
    };
    assert_eq!(huya_selection(&selected), fixed(a));
    assert_eq!(template_overrides(&selected), spelled_pool(1));
    let mut bundle = selected.clone();
    assert_eq!(
        streamer_named(&mut bundle, "Selecting")["streamer_specific_config"],
        json!({"credential_selection": {"mode": "none"}})
    );

    // A replace restores exactly the selections the bundle holds.
    set_platform_selection(json!({"mode": "inherit"})).await;
    app.put(
        &streamer_path,
        &token,
        &json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}}}),
    )
    .await
    .ok();
    import(&app, &token, &selected, "replace")
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    let mut restored = export(&app, &token).await;
    assert_eq!(huya_selection(&restored), fixed(a));
    assert_eq!(template_overrides(&restored), spelled_pool(1));
    assert_eq!(
        streamer_named(&mut restored, "Selecting")["streamer_specific_config"],
        json!({"credential_selection": {"mode": "none"}})
    );
    let effective = app
        .get_ok(
            &format!(
                "/api/credentials/selection?scope_type=streamer&scope_id={streamer}&platform_id=platform-huya"
            ),
            &token,
        )
        .await;
    assert_eq!(
        effective["resolved"]["owner"],
        json!({"type": "streamer", "streamer_id": streamer})
    );
    assert_eq!(effective["resolved"]["selection"], json!({"mode": "none"}));

    // A merge keeps the selections the bundle omits and applies the ones it
    // carries.
    let mut merge = selected.clone();
    for platform in merge["platforms"].as_array_mut().expect("platforms") {
        platform["credential_selection"] = Value::Null;
    }
    for template in merge["templates"].as_array_mut().expect("templates") {
        if template["name"] == "Selecting" {
            template["platform_overrides"] = json!({"huya": {"quality": 2}});
        }
    }
    streamer_named(&mut merge, "Selecting")["streamer_specific_config"] =
        json!({"credential_selection": fixed(b)});
    import(&app, &token, &merge, "merge")
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    let mut merged = export(&app, &token).await;
    assert_eq!(huya_selection(&merged), fixed(a));
    assert_eq!(template_overrides(&merged), spelled_pool(2));
    assert_eq!(
        streamer_named(&mut merged, "Selecting")["streamer_specific_config"],
        json!({"credential_selection": fixed(b)})
    );

    // A bundle from before accounts existed replaces every selection but
    // leaves the accounts, which it cannot carry.
    let mut legacy = selected;
    legacy["version"] = json!("0.1.8");
    legacy
        .as_object_mut()
        .expect("bundle")
        .remove("credential_profiles");
    for platform in legacy["platforms"].as_array_mut().expect("platforms") {
        platform["credential_selection"] = Value::Null;
    }
    for template in legacy["templates"].as_array_mut().expect("templates") {
        if template["name"] == "Selecting" {
            template["platform_overrides"] = json!({"huya": {"quality": 3}});
        }
    }
    streamer_named(&mut legacy, "Selecting")["streamer_specific_config"] = Value::Null;
    import(&app, &token, &legacy, "replace")
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        app.query_i64("SELECT COUNT(*) FROM credential_selections")
            .await,
        0
    );
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;
    assert_eq!(
        export(&app, &token).await["credential_profiles"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    app.shutdown().await;
}

/// A bundle may carry a streamer's settings as a JSON string, which
/// validation accepts. The import must keep those settings.
#[tokio::test]
#[ignore = "bug: the credential bundle upgrade reads a string-encoded streamer_specific_config as empty, dropping its settings and any account selection in it"]
async fn known_bug_an_import_keeps_streamer_settings_written_as_a_json_string() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    app.disabled_streamer(&token, "Encoded", "https://www.huya.com/440088")
        .await;
    let mut bundle = export(&app, &token).await;
    streamer_named(&mut bundle, "Encoded")["streamer_specific_config"] =
        json!(json!({ "record_danmu": false }).to_string());
    import(&app, &token, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    let token = sign_in_again(&app, ADMIN_PASSWORD).await;

    let mut exported = export(&app, &token).await;
    assert_eq!(
        streamer_named(&mut exported, "Encoded")["streamer_specific_config"],
        json!({ "record_danmu": false })
    );

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
    let cases: [(&str, Corrupt, &str); 16] = [
        (
            "invalid log filter directive",
            Box::new(|bundle| {
                bundle["global_config"]["log_filter_directive"] = json!("rust_srec=[unclosed")
            }),
            "log_filter_directive",
        ),
        (
            "hostless streamer URL",
            Box::new(|bundle| streamer_named(bundle, "Stable")["url"] = json!("https://")),
            "URL",
        ),
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
            "invalid cron timezone",
            Box::new(|bundle| {
                streamer_named(bundle, "Stable")["filters"] = json!([{
                    "filter_type": "CRON",
                    "config": { "expression": "0 * * * * *", "timezone": "invalid/zone" },
                }])
            }),
            "Invalid timezone",
        ),
        (
            "plaintext password hash",
            Box::new(|bundle| bundle["users"][0]["password_hash"] = json!("plaintext-password")),
            "Invalid password hash",
        ),
        (
            "scrypt password hash",
            Box::new(|bundle| {
                bundle["users"][0]["password_hash"] = json!(concat!(
                    "$scrypt$ln=17,r=8,p=1$c2FsdHNhbHRzYWx0c2FsdA$",
                    "TiYF50X49Bmdr2XJZ6yBn4ozsFFV8gGZJIXZ3Jg9pWg"
                ))
            }),
            "must use Argon2id",
        ),
        (
            "username longer than sign-in accepts",
            Box::new(|bundle| bundle["users"][0]["username"] = json!("a".repeat(129))),
            "exceeds 128 characters",
        ),
        (
            "account selection hidden in a string before schema 1.0.0",
            Box::new(|bundle| {
                bundle["version"] = json!("0.1.7");
                streamer_named(bundle, "Stable")["streamer_specific_config"] =
                    json!(r#"{"credential_selection":{"mode":"inherit"}}"#);
            }),
            "Managed credentials require backup schema 1.0.0",
        ),
        (
            "null account selection",
            Box::new(|bundle| {
                bundle["version"] = json!("1.0.0");
                streamer_named(bundle, "Stable")["streamer_specific_config"] =
                    json!({ "credential_selection": null });
            }),
            "invalid credential selection",
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
            Box::new(|bundle| bundle["version"] = json!("1.1.0")),
            "1.1.0",
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

    // Revocation does not depend on the bundle replacing any user, and cached
    // API key grants are evicted immediately when an import mutates user state.
    let mut clear_bundle = baseline.clone();
    clear_bundle
        .as_object_mut()
        .expect("bundle")
        .remove("users");
    import(&app, &second.access, &clear_bundle, "merge")
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
    let fresh_access = sign_in_again(&app, ADMIN_PASSWORD).await;
    let warm_key = app
        .create_api_key(&fresh_access, "warm before import", "full")
        .await
        .raw;
    assert_eq!(
        app.get("/api/streamers", &warm_key).await.status,
        StatusCode::OK
    );
    let mut bundle = baseline;
    bundle["users"][0]["must_change_password"] = json!(true);
    // The username limit counts characters: 128 of these take 384 bytes.
    let mut wide = bundle["users"][0].clone();
    wide["id"] = json!("wide-user");
    wide["username"] = json!("用".repeat(128));
    wide["email"] = Value::Null;
    wide["roles"] = json!(["user"]);
    bundle["users"].as_array_mut().expect("users").push(wide);
    import(&app, &fresh_access, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    app.get("/api/streamers", &warm_key)
        .await
        .assert_error(StatusCode::FORBIDDEN, "PASSWORD_CHANGE_REQUIRED");

    app.shutdown().await;
}

#[tokio::test]
async fn an_import_with_a_broken_account_graph_is_rejected_and_changes_nothing() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let account = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-huya",
                "label": "Huya main",
                "enabled": true,
                "material": { "cookies": "session=cookie-secret" },
            }),
        )
        .await
        .expect_status(StatusCode::OK);
    let mut huya = app
        .get_ok("/api/config/platforms/platform-huya", &token)
        .await;
    huya["credential_selection"] =
        json!(json!({ "mode": "fixed", "credential_id": id_of(&account.body) }).to_string());
    app.put("/api/config/platforms/platform-huya", &token, &huya)
        .await
        .expect_status(StatusCode::OK);
    let baseline = export(&app, &token).await;

    type Corrupt = fn(&mut Value);
    let cases: [(&str, Corrupt, &str); 2] = [
        (
            "selected account missing from the bundle",
            |bundle| bundle["credential_profiles"] = json!([]),
            "omits a referenced credential profile",
        ),
        (
            "account platform missing from the bundle",
            |bundle| {
                bundle["platforms"]
                    .as_array_mut()
                    .expect("platforms")
                    .retain(|platform| platform["platform_name"] != "huya")
            },
            "omits a credential profile platform",
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

    // The token still works, so nothing was revoked, and nothing was written.
    let after = export(&app, &token).await;
    assert_same_bundle(&after, &baseline);

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

/// An email channel can take no proxy route. A merge that gives a stored,
/// proxied webhook's name to an email channel without a route makes it
/// inherit; one with a proxy route is refused as invalid, not as a failure.
#[tokio::test]
async fn a_merged_email_channel_never_keeps_a_proxy_route() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let proxy = app
        .post(
            "/api/proxies",
            &token,
            &json!({"name": "Relay", "url": "http://127.0.0.1:3128"}),
        )
        .await
        .created();
    app.post(
        "/api/notifications/channels",
        &token,
        &json!({
            "name": "Alerts",
            "channel_type": "Webhook",
            "settings": {"url": "http://127.0.0.1:9/hook"},
            "proxy_route": {"kind": "proxy", "id": proxy["id"]},
        }),
    )
    .await
    .created();
    let mut bundle = export(&app, &token).await;
    let email = json!({
        "name": "Alerts",
        "channel_type": "Email",
        "settings": {
            "smtp_host": "127.0.0.1",
            "smtp_port": 25,
            "username": "",
            "password": "",
            "from_address": "srec@example.com",
            "to_addresses": ["ops@example.com"],
        },
        "subscriptions": [],
    });

    let mut routed = email.clone();
    routed["proxy_route"] = json!({"kind": "proxy", "name": "Relay"});
    bundle["notification_channels"] = json!([routed]);
    import(&app, &token, &bundle, "merge")
        .await
        .assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    bundle["notification_channels"] = json!([email]);
    import(&app, &token, &bundle, "merge")
        .await
        .expect_status(StatusCode::OK);
    // An import signs every session out.
    let token = app
        .login_tokens(ADMIN_USERNAME, ADMIN_PASSWORD)
        .await
        .access;
    let channels = app.get_ok("/api/notifications/channels", &token).await;
    assert_eq!(channels[0]["channel_type"], "Email");
    assert_eq!(channels[0]["proxy_route"], json!({"kind": "inherit"}));

    app.shutdown().await;
}
