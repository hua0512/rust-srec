//! Streamers and their filters: creation, the list the streamers page reads,
//! batch actions, partial updates, deletion and the per-streamer side routes.
//!
//! Every streamer here is either created disabled or seeded in a state the
//! scheduler never monitors (ERROR, FATAL_ERROR, OUT_OF_SPACE, NOT_FOUND,
//! CANCELLED, DISABLED), so no check ever reaches a platform. For the same
//! reason nothing here calls `clear-error` on an existing streamer or sends
//! `enabled: true` to a state it would move to NOT_LIVE: both would start
//! monitoring.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{TestApp, assert_recent, id_of, millis, now_ms};

const STATES: [&str; 11] = [
    "NOT_LIVE",
    "LIVE",
    "OUT_OF_SCHEDULE",
    "OUT_OF_SPACE",
    "FATAL_ERROR",
    "CANCELLED",
    "NOT_FOUND",
    "INSPECTING_LIVE",
    "TEMPORAL_DISABLED",
    "ERROR",
    "DISABLED",
];

async fn create_template(app: &TestApp, token: &str, name: &str) -> String {
    let template = app
        .post("/api/templates", token, &json!({ "name": name }))
        .await
        .created();
    id_of(&template)
}

async fn list_page(app: &TestApp, token: &str, query: &str) -> Value {
    app.get_ok(&format!("/api/streamers?{query}"), token).await
}

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| item["name"].as_str().expect("name").to_string())
        .collect()
}

/// A streamer row inserted before boot, for states only a platform check
/// could otherwise produce.
fn seeded_streamer(
    id: &str,
    name: &str,
    url: &str,
    platform: &str,
    state: &str,
    priority: &str,
) -> String {
    format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, priority) \
         VALUES ('{id}', '{name}', '{url}', '{platform}', '{state}', '{priority}');\n"
    )
}

#[tokio::test]
async fn create_derives_the_platform_from_the_url_and_rejects_malformed_urls() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    // The URL decides the platform; a client-supplied one is ignored.
    let created = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Bili",
                "url": "https://live.bilibili.com/4063253",
                "platform_config_id": "platform-twitch",
                "priority": "HIGH",
                "enabled": false,
                "streamer_specific_config": { "output_folder": "/recordings/bili" },
            }),
        )
        .await
        .created();
    assert_eq!(created["platform_config_id"], "platform-bilibili");
    assert_eq!(created["state"], "DISABLED");
    assert_eq!(created["enabled"], false);
    assert_eq!(created["priority"], "HIGH");
    assert_eq!(
        created["streamer_specific_config"]["output_folder"],
        "/recordings/bili"
    );

    // Detection ignores the host's case; the URL is stored as given.
    for (url, platform) in [
        ("https://LIVE.BILIBILI.COM/5050", "platform-bilibili"),
        ("https://play.sooplive.co.kr/soopuser", "platform-soop"),
        ("https://WWW.TWITCH.TV/SomeOne", "platform-twitch"),
    ] {
        let created = app
            .create_disabled_streamer(&token, json!({ "name": url, "url": url }))
            .await;
        assert_eq!(created["platform_config_id"], platform, "{url}");
        assert_eq!(created["url"], url);
    }

    for url in ["", "not-a-url", "https://live.bilibili.com/1 2"] {
        let response = app
            .post(
                "/api/streamers",
                &token,
                &json!({ "name": "Bad", "url": url, "enabled": false }),
            )
            .await;
        response.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    assert_eq!(list_page(&app, &token, "").await["total"], 4);

    // The add-streamer dialog offers exactly the config the URL belongs to.
    let metadata = app
        .post(
            "/api/streamers/extract-metadata",
            &token,
            &json!({ "url": "https://live.bilibili.com/4063253/" }),
        )
        .await
        .ok();
    assert_eq!(metadata["platform"], "Bilibili");
    assert_eq!(metadata["channel_id"], "4063253");
    let configs: Vec<&str> = metadata["valid_platform_configs"]
        .as_array()
        .expect("valid_platform_configs")
        .iter()
        .map(|config| config["id"].as_str().expect("config id"))
        .collect();
    assert_eq!(configs, ["platform-bilibili"]);

    let response = app
        .post(
            "/api/streamers/extract-metadata",
            &token,
            &json!({ "url": "not-a-url" }),
        )
        .await;
    response.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    app.shutdown().await;
}

/// The fields `StreamerSchema` in the frontend requires, with the types it
/// accepts.
#[track_caller]
fn assert_streamer_schema(streamer: &Value) {
    let fields = streamer.as_object().expect("streamer object");
    for key in [
        "id",
        "name",
        "url",
        "platform_config_id",
        "state",
        "priority",
        "enabled",
        "consecutive_error_count",
        "created_at",
        "updated_at",
    ] {
        assert!(fields.contains_key(key), "{key} missing from {streamer}");
    }
    let url = streamer["url"].as_str().expect("url string");
    assert!(reqwest::Url::parse(url).is_ok(), "{url} is not a URL");
    let state = streamer["state"].as_str().expect("state string");
    assert!(STATES.contains(&state), "unknown state {state}");
    let priority = streamer["priority"].as_str().expect("priority string");
    assert!(["HIGH", "NORMAL", "LOW"].contains(&priority));
    assert_eq!(streamer["enabled"], state != "DISABLED");
    assert!(streamer["consecutive_error_count"].is_i64());
    millis(&streamer["created_at"]);
    millis(&streamer["updated_at"]);
    for key in ["disabled_until", "last_live_time"] {
        if !streamer[key].is_null() {
            millis(&streamer[key]);
        }
    }
    let specific = &streamer["streamer_specific_config"];
    assert!(specific.is_null() || specific.is_object(), "{specific}");
}

#[tokio::test]
async fn streamer_json_matches_the_frontend_schema() {
    let disabled_until = now_ms() + 3_600_000;
    let last_live = now_ms() - 86_400_000;
    let app = TestApp::start_seeded(&format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, priority, \
             consecutive_error_count, last_error, disabled_until, last_live_time, avatar, \
             streamer_specific_config) \
         VALUES ('fatal', 'Fatal', 'https://www.huya.com/fatalroom', 'platform-huya', \
             'FATAL_ERROR', 'LOW', 3, 'room not found', {disabled_until}, {last_live}, \
             'https://example.invalid/avatar.png', '{{broken');"
    ))
    .await;
    let token = app.admin_token().await;

    let created = app
        .create_disabled_streamer(
            &token,
            json!({ "name": "Fresh", "url": "https://live.bilibili.com/777" }),
        )
        .await;
    assert_streamer_schema(&created);
    let fresh = app.streamer(&token, &id_of(&created)).await;
    assert_streamer_schema(&fresh);
    assert_eq!(fresh["state"], "DISABLED");
    assert_eq!(fresh["enabled"], false);
    assert_eq!(fresh["priority"], "NORMAL");
    assert_eq!(fresh["consecutive_error_count"], 0);
    for key in [
        "template_id",
        "avatar_url",
        "disabled_until",
        "last_error",
        "last_live_time",
        "credential_blocked",
        "streamer_specific_config",
    ] {
        assert!(fresh[key].is_null(), "{key} should be null: {fresh}");
    }
    assert_recent(millis(&fresh["created_at"]));

    // An errored streamer is still "enabled" (only DISABLED is not), carries
    // its error fields, and a stored config that is not JSON reads as null
    // rather than failing the response.
    let fatal = app.streamer(&token, "fatal").await;
    assert_streamer_schema(&fatal);
    assert_eq!(fatal["state"], "FATAL_ERROR");
    assert_eq!(fatal["enabled"], true);
    assert_eq!(fatal["priority"], "LOW");
    assert_eq!(fatal["consecutive_error_count"], 3);
    assert_eq!(fatal["last_error"], "room not found");
    assert_eq!(fatal["avatar_url"], "https://example.invalid/avatar.png");
    assert_eq!(millis(&fatal["disabled_until"]), disabled_until);
    assert_eq!(millis(&fatal["last_live_time"]), last_live);
    assert!(fatal["credential_blocked"].is_null());
    assert!(fatal["streamer_specific_config"].is_null());

    let page = list_page(&app, &token, "").await;
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    for item in items {
        assert_streamer_schema(item);
        let single = if item["id"] == "fatal" {
            &fatal
        } else {
            &fresh
        };
        for key in ["state", "enabled", "last_error", "disabled_until", "url"] {
            assert_eq!(item[key], single[key], "list and detail disagree on {key}");
        }
    }

    app.shutdown().await;
}

#[tokio::test]
async fn batch_actions_validate_up_front_and_report_a_result_per_item() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let first = app
        .disabled_streamer(&token, "First", "https://live.bilibili.com/1001")
        .await;
    let second = app
        .disabled_streamer(&token, "Second", "https://live.bilibili.com/1002")
        .await;
    let batch = |ids: Value, action: Value| {
        let (app, token) = (&app, &token);
        let body = json!({ "ids": ids, "action": action });
        async move { app.post("/api/streamers/batch", token, &body).await }
    };

    let too_many: Vec<String> = (0..=100).map(|index| format!("id-{index}")).collect();
    for ids in [
        json!([]),
        json!(too_many),
        json!([first, first]),
        json!([first, "  "]),
    ] {
        let response = batch(ids, json!({ "type": "set_priority", "priority": "HIGH" })).await;
        response.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    assert_eq!(app.streamer(&token, &first).await["priority"], "NORMAL");

    // An unknown template is refused before any streamer is touched.
    let response = batch(
        json!([first, second]),
        json!({ "type": "set_template", "template_id": "no-such-template" }),
    )
    .await;
    response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert!(app.streamer(&token, &first).await["template_id"].is_null());

    // An unknown id fails alone; the others still apply, in request order.
    let template = create_template(&app, &token, "Batch template").await;
    let result = batch(
        json!([first, "missing-streamer", second]),
        json!({ "type": "set_template", "template_id": template }),
    )
    .await
    .ok();
    assert_eq!(result["requested"], 3);
    assert_eq!(result["succeeded"], 2);
    assert_eq!(result["failed"], 1);
    let results = result["results"].as_array().expect("results");
    let ids: Vec<&str> = results
        .iter()
        .map(|item| item["id"].as_str().expect("result id"))
        .collect();
    assert_eq!(ids, [first.as_str(), "missing-streamer", second.as_str()]);
    // `BatchItemResultSchema` uses `.optional()`, which rejects `null`, so a
    // success carries no `code` or `error` key at all.
    for success in [&results[0], &results[2]] {
        assert_eq!(success["success"], true);
        let keys = success.as_object().expect("result object");
        assert!(
            !keys.contains_key("code") && !keys.contains_key("error"),
            "{success}"
        );
    }
    assert_eq!(results[1]["success"], false);
    assert_eq!(results[1]["code"], "NOT_FOUND");
    assert!(results[1]["error"].is_string());
    for id in [&first, &second] {
        assert_eq!(app.streamer(&token, id).await["template_id"], template);
    }

    let cleared = batch(
        json!([first]),
        json!({ "type": "set_template", "template_id": null }),
    )
    .await
    .ok();
    assert_eq!(cleared["succeeded"], 1);
    assert!(app.streamer(&token, &first).await["template_id"].is_null());
    assert_eq!(app.streamer(&token, &second).await["template_id"], template);

    let prioritised = batch(
        json!([first, second]),
        json!({ "type": "set_priority", "priority": "LOW" }),
    )
    .await
    .ok();
    assert_eq!(prioritised["succeeded"], 2);
    let disabled = batch(
        json!([first]),
        json!({ "type": "set_enabled", "enabled": false }),
    )
    .await
    .ok();
    assert_eq!(disabled["succeeded"], 1);
    let first_now = app.streamer(&token, &first).await;
    assert_eq!(first_now["priority"], "LOW");
    assert_eq!(first_now["state"], "DISABLED");

    let deleted = batch(json!([first, second]), json!({ "type": "delete" }))
        .await
        .ok();
    assert_eq!(deleted["succeeded"], 2);
    assert_eq!(list_page(&app, &token, "").await["total"], 0);
    for id in [&first, &second] {
        let response = app.get(&format!("/api/streamers/{id}"), &token).await;
        response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    app.shutdown().await;
}

#[tokio::test]
async fn list_filters_sorts_and_paginates_like_the_streamers_page() {
    // States the scheduler never monitors, so nothing is checked.
    // (id, platform, state, priority); each name is the capitalized id.
    let rows = [
        ("yankee", "bilibili", "ERROR", "HIGH"),
        ("echo", "bilibili", "ERROR", "NORMAL"),
        ("foxtrot", "twitch", "FATAL_ERROR", "HIGH"),
        ("oscar", "twitch", "OUT_OF_SPACE", "LOW"),
        ("november", "huya", "NOT_FOUND", "NORMAL"),
        ("charlie", "huya", "CANCELLED", "NORMAL"),
        ("zulu", "bilibili", "DISABLED", "HIGH"),
        ("bravo", "bilibili", "DISABLED", "NORMAL"),
        ("delta", "twitch", "DISABLED", "NORMAL"),
        ("alpha", "huya", "DISABLED", "LOW"),
    ];
    let seed: String = rows
        .iter()
        .enumerate()
        .map(|(index, (id, platform, state, priority))| {
            let name = format!("{}{}", id[..1].to_uppercase(), &id[1..]);
            let url = match *platform {
                "bilibili" => format!("https://live.bilibili.com/{}", 2001 + index),
                "twitch" => format!("https://www.twitch.tv/{id}"),
                _ => format!("https://www.huya.com/{id}"),
            };
            let platform = format!("platform-{platform}");
            seeded_streamer(id, &name, &url, &platform, state, priority)
        })
        .collect();
    let app = TestApp::start_seeded(&seed).await;
    let token = app.admin_token().await;
    let template = create_template(&app, &token, "Night shift").await;
    app.put(
        "/api/streamers/zulu",
        &token,
        &json!({ "template_id": template }),
    )
    .await
    .ok();

    // Errors first (ERROR, FATAL_ERROR, OUT_OF_SPACE, NOT_FOUND), then
    // CANCELLED, then DISABLED; priority descending, then name, within a state.
    let all = list_page(&app, &token, "").await;
    assert_eq!(all["total"], 10);
    assert_eq!(all["limit"], 20);
    assert_eq!(all["offset"], 0);
    assert_eq!(
        names(&all),
        [
            "Yankee", "Echo", "Foxtrot", "Oscar", "November", "Charlie", "Zulu", "Bravo", "Delta",
            "Alpha"
        ]
    );

    let page = list_page(&app, &token, "limit=3&offset=2").await;
    assert_eq!(names(&page), ["Foxtrot", "Oscar", "November"]);
    assert_eq!(page["total"], 10);
    assert_eq!(page["limit"], 3);
    assert_eq!(page["offset"], 2);
    let capped = list_page(&app, &token, "limit=500").await;
    assert_eq!(capped["limit"], 100);
    assert_eq!(capped["total"], 10);

    let cases: [(&str, &[&str]); 12] = [
        ("search=ECHO", &["Echo"]),
        ("search=twitch.tv", &["Foxtrot", "Oscar", "Delta"]),
        ("platform=platform-huya", &["November", "Charlie", "Alpha"]),
        ("state=error,fatal_error", &["Yankee", "Echo", "Foxtrot"]),
        ("state=NOT_A_STATE", &[]),
        ("enabled=false", &["Zulu", "Bravo", "Delta", "Alpha"]),
        (
            "enabled=true",
            &["Yankee", "Echo", "Foxtrot", "Oscar", "November", "Charlie"],
        ),
        ("priority=HIGH", &["Yankee", "Foxtrot", "Zulu"]),
        ("template=TEMPLATE", &["Zulu"]),
        (
            "template_unassigned=true&state=DISABLED",
            &["Bravo", "Delta", "Alpha"],
        ),
        (
            "sort_by=name&sort_dir=desc&limit=4",
            &["Zulu", "Yankee", "Oscar", "November"],
        ),
        (
            "sort_by=priority&platform=platform-bilibili",
            &["Bravo", "Echo", "Yankee", "Zulu"],
        ),
    ];
    for (query, expected) in cases {
        let query = query.replace("TEMPLATE", &template);
        let page = list_page(&app, &token, &query).await;
        assert_eq!(names(&page), expected, "{query}");
        if !query.contains("limit") {
            assert_eq!(page["total"], expected.len(), "{query}");
        }
    }

    let configs = [
        (
            "alpha",
            json!({"proxy_route": {"kind": "direct"}, "credential_selection": {"mode": "none"}}),
        ),
        ("bravo", json!({})),
        ("delta", json!({"proxy_route": {"kind": "system"}})),
    ];
    for (id, config) in &configs {
        app.put(
            &format!("/api/streamers/{id}"),
            &token,
            &json!({"streamer_specific_config": config}),
        )
        .await
        .ok();
    }
    for (offset, (id, config)) in configs.iter().enumerate() {
        let page = list_page(
            &app,
            &token,
            &format!("state=DISABLED&sort_by=name&limit=1&offset={offset}"),
        )
        .await;
        assert_eq!(page["total"], 4);
        assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(page["items"][0]["id"], *id);
        let actual = &page["items"][0]["streamer_specific_config"];
        if *id == "bravo" {
            assert!(actual.get("proxy_route").is_none());
            assert!(actual.get("credential_selection").is_none());
        } else {
            assert_eq!(actual, config);
        }
    }
    assert!(
        list_page(&app, &token, "state=DISABLED&sort_by=name&limit=1&offset=4").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    app.shutdown().await;
}

#[tokio::test]
async fn deleting_a_streamer_hides_it_at_once_and_removes_what_it_owned() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let account = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-bilibili",
                "label": "Main account",
                "enabled": true,
                "material": { "cookies": "SESSDATA=e2e" },
            }),
        )
        .await
        .ok();
    let account_id = id_of(&account);
    let streamer = app
        .create_disabled_streamer(
            &token,
            json!({
                "name": "Doomed",
                "url": "https://live.bilibili.com/3001",
                "streamer_specific_config": {
                    "credential_selection": { "mode": "fixed", "credential_id": account_id },
                },
            }),
        )
        .await;
    let id = id_of(&streamer);
    let path = format!("/api/streamers/{id}");
    assert_eq!(
        streamer["streamer_specific_config"]["credential_selection"]["credential_id"],
        account_id
    );
    let kept = app
        .disabled_streamer(&token, "Kept", "https://live.bilibili.com/3002")
        .await;

    app.post(
        &format!("{path}/filters"),
        &token,
        &json!({
            "streamer_id": id,
            "filter_type": "KEYWORD",
            "config": { "include": ["live"], "exclude": [] },
        }),
    )
    .await
    .created();
    app.run_sql(&format!(
        "INSERT INTO streamer_check_history (streamer_id, checked_at, duration_ms, outcome) \
         VALUES ('{id}', {}, 12, 'offline');",
        now_ms()
    ))
    .await;
    assert_eq!(
        app.get_ok(&format!("{path}/check-history"), &token).await["items"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // The account cannot go while a streamer selects it.
    let delete_account = || {
        let (app, token) = (&app, &token);
        let path = format!("/api/credentials/profiles/{account_id}");
        let body = json!({ "expected_version": account["version"] });
        async move {
            app.send(reqwest::Method::DELETE, &path, Some(token), Some(&body))
                .await
        }
    };
    let refused = delete_account().await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);

    let deleted = app.delete(&path, &token).await.ok();
    assert_eq!(deleted["success"], true);
    assert!(deleted["message"].is_string());

    // Gone for every route at once.
    for response in [
        app.get(&path, &token).await,
        app.put(&path, &token, &json!({ "name": "Back" })).await,
        app.post(&format!("{path}/clear-error"), &token, &json!({}))
            .await,
        app.patch(
            &format!("{path}/priority"),
            &token,
            &json!({ "priority": "HIGH" }),
        )
        .await,
        app.get(&format!("{path}/check-history"), &token).await,
        app.delete(&path, &token).await,
    ] {
        response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let remaining = list_page(&app, &token, "").await;
    assert_eq!(remaining["total"], 1);
    assert_eq!(remaining["items"][0]["id"], kept.as_str());

    // With nothing in flight the row is reaped inside the DELETE, so its
    // filters and check history are already gone and the account it selected
    // is free to delete.
    for rows in [
        format!("streamers WHERE id = '{id}'"),
        format!("filters WHERE streamer_id = '{id}'"),
        format!("streamer_check_history WHERE streamer_id = '{id}'"),
    ] {
        let count = app.query_i64(&format!("SELECT COUNT(*) FROM {rows}")).await;
        assert_eq!(count, 0, "{rows}");
    }
    assert_eq!(
        app.get_ok(&format!("{path}/filters"), &token).await,
        json!([])
    );
    delete_account().await.ok();

    app.shutdown().await;
}

#[tokio::test]
async fn duplicate_urls_are_refused_case_insensitively_until_the_owner_is_reaped() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let first = app
        .disabled_streamer(&token, "First", "https://live.bilibili.com/4001")
        .await;
    let second = app
        .disabled_streamer(&token, "Second", "https://live.bilibili.com/4002")
        .await;
    // Creating a streamer at `url`, and moving `second` there, are both refused.
    let assert_taken = |url: &'static str| {
        let (app, token, second) = (&app, &token, &second);
        async move {
            let body = json!({ "name": "Copy", "url": url, "enabled": false });
            let created = app.post("/api/streamers", token, &body).await;
            created.assert_error(StatusCode::CONFLICT, "CONFLICT");
            let path = format!("/api/streamers/{second}");
            let moved = app.put(&path, token, &json!({ "url": url })).await;
            moved.assert_error(StatusCode::CONFLICT, "CONFLICT");
        }
    };

    assert_taken("https://LIVE.BILIBILI.COM/4001").await;
    assert_eq!(
        app.streamer(&token, &second).await["url"],
        "https://live.bilibili.com/4002"
    );
    assert_eq!(list_page(&app, &token, "").await["total"], 2);

    // A streamer with nothing in flight is reaped inside the DELETE, so its
    // URL is free again.
    app.delete(&format!("/api/streamers/{first}"), &token)
        .await
        .ok();
    let recreated = app
        .create_disabled_streamer(
            &token,
            json!({ "name": "Again", "url": "https://live.bilibili.com/4001" }),
        )
        .await;
    assert_eq!(recreated["name"], "Again");

    // An ended session whose final pipeline was never settled keeps a deleted
    // streamer marked, and its URL stays reserved while it is.
    let retiring = app
        .disabled_streamer(&token, "Retiring", "https://live.bilibili.com/4003")
        .await;
    app.run_sql(&format!(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             total_size_bytes, session_complete_dispatched) \
         VALUES ('unsettled', '{retiring}', 'Retiring', {}, {}, 1024, 0);",
        now_ms() - 120_000,
        now_ms() - 60_000
    ))
    .await;
    app.delete(&format!("/api/streamers/{retiring}"), &token)
        .await
        .ok();
    app.get(&format!("/api/streamers/{retiring}"), &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(
        app.query_i64(&format!(
            "SELECT COUNT(*) FROM streamers WHERE id = '{retiring}' AND deleted_at IS NOT NULL"
        ))
        .await,
        1
    );
    assert_taken("https://LIVE.bilibili.com/4003").await;

    app.shutdown().await;
}

#[tokio::test]
async fn put_is_a_partial_update_and_disabling_clears_the_error_backoff() {
    let disabled_until = now_ms() + 3_600_000;
    let app = TestApp::start_seeded(&format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, priority, \
             consecutive_error_count, last_error, disabled_until) VALUES \
         ('errored', 'Errored', 'https://www.huya.com/errored', 'platform-huya', 'ERROR', \
             'NORMAL', 0, NULL, NULL), \
         ('cancelled', 'Cancelled', 'https://www.huya.com/cancelled', 'platform-huya', \
             'CANCELLED', 'NORMAL', 0, NULL, NULL), \
         ('fatal', 'Fatal', 'https://www.huya.com/fatal', 'platform-huya', 'FATAL_ERROR', \
             'NORMAL', 4, 'banned', {disabled_until});"
    ))
    .await;
    let token = app.admin_token().await;
    let template = create_template(&app, &token, "Partial").await;

    // Create and update refuse an unknown template, as the batch route does.
    let orphan = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Orphan",
                "url": "https://live.bilibili.com/6001",
                "template_id": "no-such-template",
                "enabled": false,
            }),
        )
        .await;
    orphan.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let created = app
        .post(
            "/api/streamers",
            &token,
            &json!({
                "name": "Original",
                "url": "https://live.bilibili.com/5001",
                "template_id": template,
                "priority": "HIGH",
                "enabled": false,
                "streamer_specific_config": { "output_folder": "/keep" },
            }),
        )
        .await
        .created();
    let id = id_of(&created);
    let path = format!("/api/streamers/{id}");
    let unknown = app
        .put(&path, &token, &json!({ "template_id": "no-such-template" }))
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(list_page(&app, &token, "").await["total"], 4);

    // Omitted fields are kept, including the template.
    let renamed = app
        .put(&path, &token, &json!({ "name": "Renamed" }))
        .await
        .ok();
    assert_eq!(renamed["name"], "Renamed");
    assert_eq!(renamed["template_id"], template);
    assert_eq!(renamed["priority"], "HIGH");
    assert_eq!(renamed["url"], "https://live.bilibili.com/5001");
    assert_eq!(
        renamed["streamer_specific_config"]["output_folder"],
        "/keep"
    );
    assert_eq!(renamed["state"], "DISABLED");

    // An explicit null clears the template; nothing else moves.
    let cleared = app
        .put(
            &path,
            &token,
            &json!({ "template_id": null, "priority": "LOW" }),
        )
        .await
        .ok();
    assert!(cleared["template_id"].is_null());
    assert_eq!(cleared["priority"], "LOW");
    assert_eq!(cleared["name"], "Renamed");

    // A new URL re-derives the platform.
    let moved = app
        .put(
            &path,
            &token,
            &json!({ "url": "https://www.twitch.tv/moved" }),
        )
        .await
        .ok();
    assert_eq!(moved["platform_config_id"], "platform-twitch");
    let stored = app.streamer(&token, &id).await;
    assert_eq!(stored["url"], "https://www.twitch.tv/moved");
    assert_eq!(stored["platform_config_id"], "platform-twitch");
    assert_eq!(stored["name"], "Renamed");
    assert!(stored["template_id"].is_null());

    let response = app.put(&path, &token, &json!({ "url": "not-a-url" })).await;
    response.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(
        app.streamer(&token, &id).await["url"],
        "https://www.twitch.tv/moved"
    );

    // The priority route sets the priority and refuses an unknown one.
    let priority = format!("{path}/priority");
    let patched = app
        .patch(&priority, &token, &json!({ "priority": "HIGH" }))
        .await
        .ok();
    assert_eq!(patched["priority"], "HIGH");
    assert_eq!(patched["state"], "DISABLED");
    let invalid = app
        .patch(&priority, &token, &json!({ "priority": "URGENT" }))
        .await;
    assert_eq!(
        invalid.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        invalid.text
    );
    assert_eq!(app.streamer(&token, &id).await["priority"], "HIGH");

    // Disabling clears the error backoff along with the state.
    let fatal = app.streamer(&token, "fatal").await;
    assert_eq!(fatal["consecutive_error_count"], 4);
    assert_eq!(millis(&fatal["disabled_until"]), disabled_until);
    let disabled = app
        .put("/api/streamers/fatal", &token, &json!({ "enabled": false }))
        .await
        .ok();
    assert_eq!(disabled["state"], "DISABLED");
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["consecutive_error_count"], 0);
    assert!(disabled["disabled_until"].is_null());
    assert!(disabled["last_error"].is_null());
    let fatal = app.streamer(&token, "fatal").await;
    assert_eq!(fatal["state"], "DISABLED");
    assert_eq!(fatal["consecutive_error_count"], 0);
    assert!(fatal["disabled_until"].is_null());

    // `enabled: true` leaves ERROR and CANCELLED as they are: enabling moves
    // only DISABLED or an error state to NOT_LIVE, CANCELLED is neither, and
    // ERROR may not make that transition.
    for (id, state) in [("errored", "ERROR"), ("cancelled", "CANCELLED")] {
        let response = app
            .put(
                &format!("/api/streamers/{id}"),
                &token,
                &json!({ "enabled": true }),
            )
            .await
            .ok();
        assert_eq!(response["state"], state);
        assert_eq!(app.streamer(&token, id).await["state"], state);
    }

    let response = app
        .put(
            "/api/streamers/no-such-streamer",
            &token,
            &json!({ "name": "x" }),
        )
        .await;
    response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn check_history_is_oldest_first_and_clamped() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let id = app
        .disabled_streamer(&token, "Watched", "https://live.bilibili.com/7001")
        .await;
    let history = format!("/api/streamers/{id}/check-history");

    assert_eq!(app.get_ok(&history, &token).await, json!({ "items": [] }));
    app.get("/api/streamers/no-such-streamer/check-history", &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // 205 rows one second apart; the newest two carry stream telemetry, one
    // of them malformed.
    let base = now_ms() - 3_600_000;
    app.run_sql(&format!(
        r#"WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 203)
        INSERT INTO streamer_check_history (streamer_id, checked_at, duration_ms, outcome)
        SELECT '{id}', {base} + i * 1000, i, 'offline' FROM n;
        INSERT INTO streamer_check_history (streamer_id, checked_at, duration_ms, outcome,
            streams_extracted, stream_selected, streams_extracted_json, title, category, viewer_count)
        VALUES ('{id}', {}, 204, 'live', 2, '{{not json', '[broken', 'Broken telemetry', 'Games', 10),
            ('{id}', {}, 205, 'live', 1, '{{"quality":"1080p"}}', '[{{"quality":"1080p"}}]',
                'Live now', 'Games', 42);"#,
        base + 204_000,
        base + 205_000
    ))
    .await;

    let page = app.get_ok(&history, &token).await;
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 60);
    let durations: Vec<i64> = items
        .iter()
        .map(|item| item["duration_ms"].as_i64().expect("duration_ms"))
        .collect();
    assert_eq!(durations, (146..=205).collect::<Vec<_>>());
    let broken = &items[58];
    assert_eq!(broken["outcome"], "live");
    assert_eq!(broken["title"], "Broken telemetry");
    assert!(broken["stream_selected"].is_null());
    assert!(broken["streams_extracted_detail"].is_null());
    let newest = &items[59];
    assert_eq!(newest["stream_selected"], json!({ "quality": "1080p" }));
    assert_eq!(
        newest["streams_extracted_detail"],
        json!([{ "quality": "1080p" }])
    );
    assert_eq!(newest["viewer_count"], 42);
    assert_eq!(millis(&newest["checked_at"]), base + 205_000);

    for (limit, expected) in [("0", 1), ("-5", 1), ("3", 3), ("1000", 200)] {
        let page = app
            .get_ok(&format!("{history}?limit={limit}"), &token)
            .await;
        let items = page["items"].as_array().expect("items");
        assert_eq!(items.len(), expected, "limit={limit}");
        assert_eq!(items.last().expect("newest")["duration_ms"], 205);
    }

    app.shutdown().await;
}

#[tokio::test]
async fn filters_belong_to_the_streamer_in_their_path() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let owner = app
        .disabled_streamer(&token, "Owner", "https://live.bilibili.com/8001")
        .await;
    let other = app
        .disabled_streamer(&token, "Other", "https://live.bilibili.com/8002")
        .await;
    let filters = format!("/api/streamers/{owner}/filters");
    let keyword = |streamer_id: &str| {
        json!({
            "streamer_id": streamer_id,
            "filter_type": "KEYWORD",
            "config": { "include": ["live"], "exclude": ["rerun"] },
        })
    };

    let unknown_streamer = app
        .post(
            "/api/streamers/no-such-streamer/filters",
            &token,
            &keyword("no-such-streamer"),
        )
        .await;
    unknown_streamer.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let mismatched = app.post(&filters, &token, &keyword(&other)).await;
    mismatched.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let unknown = app
        .post(
            &filters,
            &token,
            &json!({ "streamer_id": owner, "filter_type": "MOOD", "config": {} }),
        )
        .await;
    unknown.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let created = app.post(&filters, &token, &keyword(&owner)).await.created();
    assert_eq!(created["streamer_id"], owner.as_str());
    let own = format!("{filters}/{}", id_of(&created));
    assert_eq!(app.get_ok(&filters, &token).await, json!([created]));
    assert_eq!(
        app.get_ok(&format!("/api/streamers/{other}/filters"), &token)
            .await,
        json!([])
    );

    // Another streamer's path cannot see, change or remove the filter.
    let foreign = format!("/api/streamers/{other}/filters/{}", id_of(&created));
    for response in [
        app.get(&foreign, &token).await,
        app.patch(
            &foreign,
            &token,
            &json!({ "config": { "include": ["x"], "exclude": [] } }),
        )
        .await,
        app.delete(&foreign, &token).await,
    ] {
        response.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    assert_eq!(app.get_ok(&own, &token).await["config"], created["config"]);

    // Changing the type needs a config for the new type.
    let response = app
        .patch(&own, &token, &json!({ "filter_type": "CATEGORY" }))
        .await;
    response.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(app.get_ok(&own, &token).await["filter_type"], "KEYWORD");
    let retyped = app
        .patch(
            &own,
            &token,
            &json!({ "filter_type": "CATEGORY", "config": { "categories": ["Music"] } }),
        )
        .await
        .ok();
    assert_eq!(retyped["filter_type"], "CATEGORY");
    assert_eq!(retyped["config"], json!({ "categories": ["Music"] }));

    app.delete(&own, &token).await.ok();
    app.get(&own, &token)
        .await
        .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(app.get_ok(&filters, &token).await, json!([]));
    // Nothing refused above was stored for any streamer.
    assert_eq!(app.query_i64("SELECT COUNT(*) FROM filters").await, 0);

    app.shutdown().await;
}

#[tokio::test]
async fn filter_configs_are_validated_and_normalized_when_saved() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let id = app
        .disabled_streamer(&token, "Filtered", "https://live.bilibili.com/9001")
        .await;
    let filters = format!("/api/streamers/{id}/filters");
    let create = |filter_type: &'static str, config: Value| {
        let (app, token, filters) = (&app, &token, &filters);
        let body = json!({ "streamer_id": id, "filter_type": filter_type, "config": config });
        async move { app.post(filters, token, &body).await }
    };

    for (filter_type, config) in [
        ("CRON", json!({ "expression": "invalid cron" })),
        ("CRON", json!({ "expression": "0 22 * * 5" })),
        ("REGEX", json!({ "pattern": "[invalid" })),
        ("REGEX", json!({ "pattern": "(unclosed" })),
        (
            "TIME_BASED",
            json!({ "days_of_week": [], "start_time": "09:00", "end_time": "10:00" }),
        ),
        (
            "TIME_BASED",
            json!({ "days_of_week": ["Funday"], "start_time": "09:00", "end_time": "10:00" }),
        ),
        (
            "TIME_BASED",
            json!({ "days_of_week": ["mon"], "start_time": "09:00", "end_time": "09:00:00" }),
        ),
        (
            "TIME_BASED",
            json!({
                "days_of_week": ["mon"], "start_time": "09:00", "end_time": "10:00",
                "timezone": "Mars/Olympus_Mons",
            }),
        ),
    ] {
        let response = create(filter_type, config.clone()).await;
        assert_eq!(
            (response.status, response.error_code()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("VALIDATION_ERROR")),
            "{filter_type} {config}: {}",
            response.text
        );
    }
    assert_eq!(app.get_ok(&filters, &token).await, json!([]));

    // Six- and seven-field cron expressions are accepted as written.
    for expression in ["0 0 22 * * 5,6", "0 30 9 * * Mon-Fri 2030"] {
        let created = create("CRON", json!({ "expression": expression }))
            .await
            .created();
        assert_eq!(created["config"]["expression"], expression);
    }

    let regex = create("REGEX", json!({ "pattern": "^Live" }))
        .await
        .created();
    assert_eq!(
        regex["config"],
        json!({ "pattern": "^Live", "case_insensitive": false, "exclude": false })
    );

    // Days and times are stored in canonical form; no timezone means UTC and
    // stores no key.
    let schedule = create(
        "TIME_BASED",
        json!({ "days_of_week": ["mon", "SAT"], "start_time": "09:00", "end_time": "17:30" }),
    )
    .await
    .created();
    let expected = json!({
        "days_of_week": ["Monday", "Saturday"],
        "start_time": "09:00:00",
        "end_time": "17:30:00",
    });
    assert_eq!(schedule["config"], expected);
    let stored = app
        .get_ok(&format!("{filters}/{}", id_of(&schedule)), &token)
        .await;
    assert_eq!(stored["config"], expected);

    let keyword = create(
        "KEYWORD",
        json!({ "include": ["  live ", "", "   "], "exclude": [" rerun", ""] }),
    )
    .await
    .created();
    assert_eq!(
        keyword["config"],
        json!({ "include": ["live"], "exclude": ["rerun"] })
    );
    let category = create("CATEGORY", json!({ "categories": [" Games ", ""] }))
        .await
        .created();
    assert_eq!(category["config"], json!({ "categories": ["Games"] }));

    // `local` is kept, and a replacement that omits the timezone keeps the
    // stored one.
    let local = create(
        "TIME_BASED",
        json!({
            "days_of_week": ["Sunday"], "start_time": "20:00", "end_time": "02:00",
            "timezone": "local",
        }),
    )
    .await
    .created();
    assert_eq!(local["config"]["timezone"], "local");
    let local_path = format!("{filters}/{}", id_of(&local));
    let replaced = app
        .patch(
            &local_path,
            &token,
            &json!({ "config": { "days_of_week": ["fri"], "start_time": "21:00", "end_time": "23:00" } }),
        )
        .await
        .ok();
    assert_eq!(
        replaced["config"],
        json!({
            "days_of_week": ["Friday"],
            "start_time": "21:00:00",
            "end_time": "23:00:00",
            "timezone": "local",
        })
    );
    assert_eq!(
        app.get_ok(&local_path, &token).await["config"]["timezone"],
        "local"
    );

    app.shutdown().await;
}
