//! Accounts on the fake platform signed in and repaired by its scripted
//! credential provider ([`FakeAccounts`]): QR login, and the refresh a
//! rejected check triggers.

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::fake_provider::FakeAccounts;
use crate::support::{FakePlatform, Seed, TestApp, id_of};

const PLATFORM: &str = "/api/config/platforms/platform-fake";

/// Make the profile `profile_id` the fake platform's fixed account.
async fn select_account(app: &TestApp, token: &str, profile_id: &str) {
    let mut platform = app.get_ok(PLATFORM, token).await;
    // The platform route carries the selection as a JSON document in a string.
    platform["credential_selection"] =
        json!(json!({ "mode": "fixed", "credential_id": profile_id }).to_string());
    app.put(PLATFORM, token, &platform)
        .await
        .expect_status(StatusCode::OK);
}

async fn poll_login(app: &TestApp, token: &str, login_id: &str) -> Value {
    app.send(
        Method::POST,
        &format!("/api/credentials/login-sessions/{login_id}/poll"),
        Some(token),
        None,
    )
    .await
    .ok()
}

#[tokio::test]
async fn a_qr_account_is_used_refreshed_and_kept_separate_from_other_accounts() {
    let accounts = FakeAccounts::install();
    let fake = FakePlatform::start().await;
    let room = fake.room("qr-login");
    room.set_live(room.live("Signed in by QR"));
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let capabilities = app
        .get_ok(
            "/api/credentials/capabilities?platform_id=platform-fake",
            &token,
        )
        .await;
    assert_eq!(capabilities["qr_login"], true, "{capabilities}");

    let generated = app
        .post(
            "/api/credentials/login-sessions",
            &token,
            &json!({ "type": "create", "platform_id": "platform-fake", "label": "QR account" }),
        )
        .await
        .ok();
    let login_id = generated["login_id"].as_str().expect("login id");
    let code = FakeAccounts::auth_code(generated["url"].as_str().expect("QR code URL"));
    let waiting = |status: &str| json!({ "status": status, "profile_id": null, "version": null });
    assert_eq!(
        poll_login(&app, &token, login_id).await,
        waiting("not_scanned")
    );
    accounts.scan(&code);
    assert_eq!(poll_login(&app, &token, login_id).await, waiting("scanned"));
    accounts.confirm(&code, "session=qr-e2e; uid=7");
    let completed = poll_login(&app, &token, login_id).await;
    assert_eq!(completed["status"], "completed", "{completed}");
    // A client whose completing poll was lost reads the same receipt again.
    assert_eq!(poll_login(&app, &token, login_id).await, completed);

    let profile_id = completed["profile_id"].as_str().expect("saved profile id");
    let detail = app
        .get_ok(&format!("/api/credentials/profiles/{profile_id}"), &token)
        .await;
    let profile = &detail["profile"];
    assert_eq!(profile["label"], "QR account");
    assert_eq!(profile["platform_config_id"], "platform-fake");
    assert_eq!(profile["enabled"], true);
    assert_eq!(profile["has_cookies"], true);
    assert_eq!(profile["version"], completed["version"]);
    assert_eq!(detail["capabilities"]["qr_login"], true);

    select_account(&app, &token, profile_id).await;
    app.post("/api/parse", &token, &json!({ "url": room.url() }))
        .await
        .expect_status(StatusCode::OK);
    let checks = fake.requests_to(&room, "status");
    assert_eq!(checks.len(), 1, "{checks:?}");
    assert_eq!(checks[0].header("cookie"), Some("session=qr-e2e; uid=7"));

    let other = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-fake", "label": "Other account", "enabled": true,
                "material": {"cookies": "session=other-qr-e2e"},
            }),
        )
        .await
        .ok();
    accounts.refresh_to("session=qr-e2e; uid=7", "session=renewed-qr-e2e", || {});
    let refreshed = app
        .post(
            &format!("/api/credentials/profiles/{profile_id}/refresh"),
            &token,
            &json!({}),
        )
        .await
        .ok();
    assert!(refreshed["profile"]["revision"].as_i64() > profile["revision"].as_i64());
    app.post("/api/parse", &token, &json!({"url": room.url()}))
        .await
        .ok();
    assert_eq!(
        fake.requests_to(&room, "status")
            .last()
            .unwrap()
            .header("cookie"),
        Some("session=renewed-qr-e2e")
    );
    select_account(&app, &token, &id_of(&other)).await;
    app.post("/api/parse", &token, &json!({"url": room.url()}))
        .await
        .ok();
    assert_eq!(
        fake.requests_to(&room, "status")
            .last()
            .unwrap()
            .header("cookie"),
        Some("session=other-qr-e2e")
    );
    let unchanged = app
        .get_ok(
            &format!("/api/credentials/profiles/{}", id_of(&other)),
            &token,
        )
        .await;
    assert_eq!(unchanged["profile"]["revision"], other["revision"]);
    assert_eq!(accounts.refreshes_of("session=other-qr-e2e"), 0);
    let backup = app.get_ok("/api/config/backup/export", &token).await;
    let profiles = backup["credential_profiles"].as_array().unwrap();
    for (label, cookies) in [
        ("QR account", "session=renewed-qr-e2e"),
        ("Other account", "session=other-qr-e2e"),
    ] {
        assert_eq!(
            profiles
                .iter()
                .find(|profile| profile["label"] == label)
                .unwrap()["cookies"],
            cookies
        );
    }

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_check_rejected_for_authentication_refreshes_the_account_and_retries_with_its_new_cookies()
 {
    let accounts = FakeAccounts::install();
    let fake = FakePlatform::start().await;
    let room = fake.room("refresh");
    room.set_error("authentication", "session expired");
    let stale = "session=stale-refresh-e2e";
    let fresh = "session=fresh-refresh-e2e";
    // The platform accepts the account again once it has been refreshed.
    let reopened = room.clone();
    accounts.refresh_to(stale, fresh, move || {
        reopened.set_live(reopened.live("Back after a refresh"));
    });
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    let created = app
        .post(
            "/api/credentials/profiles",
            &token,
            &json!({
                "platform_id": "platform-fake",
                "label": "Expiring account",
                "enabled": true,
                "material": {
                    "cookies": stale,
                    "refresh_token": null,
                    "access_token": null,
                    "reauth_config": null,
                },
            }),
        )
        .await
        .ok();
    let profile_id = id_of(&created);
    select_account(&app, &token, &profile_id).await;
    app.create_enabled_streamer(&token, json!({ "name": "Refreshed", "url": room.url() }))
        .await;

    room.wait_for_media_connection(1).await;
    let checks: Vec<_> = fake
        .requests_to(&room, "status")
        .iter()
        .map(|request| request.header("cookie").map(str::to_owned))
        .collect();
    assert!(checks.len() >= 2, "{checks:?}");
    assert_eq!(checks[0].as_deref(), Some(stale), "{checks:?}");
    assert!(
        checks[1..]
            .iter()
            .all(|cookie| cookie.as_deref() == Some(fresh)),
        "every check after the refresh uses the new cookies: {checks:?}"
    );
    let media = fake.requests_to(&room, "live.flv");
    assert_eq!(media.len(), 1, "{media:?}");
    assert_eq!(media[0].header("cookie"), Some(fresh));
    assert_eq!(accounts.refreshes_of(stale), 1);
    let refreshed = app
        .get_ok(&format!("/api/credentials/profiles/{profile_id}"), &token)
        .await;
    assert!(
        refreshed["profile"]["revision"].as_i64() > created["revision"].as_i64(),
        "the refresh stored new material: {refreshed}"
    );

    app.shutdown().await;
}
