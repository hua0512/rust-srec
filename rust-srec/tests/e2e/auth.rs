//! Authentication, sessions and API keys, driven through the real router with
//! JWT authentication enabled.

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{
    ADMIN_PASSWORD, ADMIN_USERNAME, JWT_SECRET, Response, SEEDED_ADMIN_PASSWORD, TestApp, WAIT,
    assert_closes, assert_recent, eventually, now_ms, tokens_from,
};

/// The body every failed credential check returns, whether or not the
/// username exists.
fn invalid_login_body() -> Value {
    json!({ "code": "UNAUTHORIZED", "message": "Invalid username or password" })
}

/// Send a request with explicit headers (for credentials the harness's bearer
/// helper cannot express).
async fn send_with_headers(
    app: &TestApp,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Option<&Value>,
) -> Response {
    let mut request = app.client().request(method, app.url(path)).headers(headers);
    if let Some(body) = body {
        request = request.json(body);
    }
    TestApp::read(request.send().await.expect("send the request")).await
}

fn authorization(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(value).expect("valid header value"),
    );
    headers
}

async fn logout(app: &TestApp, headers: HeaderMap, refresh_token: &str) -> Response {
    send_with_headers(
        app,
        Method::POST,
        "/api/auth/logout",
        headers,
        Some(&json!({ "refresh_token": refresh_token })),
    )
    .await
}

/// Status of `GET /api/streamers`, the canonical middleware-protected route.
async fn streamers_status(app: &TestApp, token: &str) -> StatusCode {
    app.get("/api/streamers", token).await.status
}

fn decode_claims(token: &str) -> Value {
    let mut validation = Validation::default();
    validation.set_issuer(&["rust-srec"]);
    validation.set_audience(&["rust-srec-api"]);
    jsonwebtoken::decode::<Value>(
        token,
        &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
        &validation,
    )
    .expect("decode a harness-issued token")
    .claims
}

fn mint(claims: &Value, secret: &str) -> String {
    jsonwebtoken::encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode a JWT")
}

#[tokio::test]
async fn anonymous_callers_get_401_on_every_route_except_the_public_ones() {
    let app = TestApp::start().await;

    let spec = app
        .send(Method::GET, "/api/docs/openapi.json", None, None)
        .await
        .expect_status(StatusCode::OK);
    let paths = spec.body["paths"].as_object().expect("OpenAPI paths");

    // Reachable without any credential; each is checked separately below.
    let public = [
        "/api/health/live",
        "/api/health/idle",
        "/api/auth/login",
        "/api/auth/refresh",
        "/api/auth/logout",
        "/api/logging/archive",
        "/api/logging/download",
    ];
    let mut checked = 0;
    for (path, operations) in paths {
        if public.contains(&path.as_str()) {
            continue;
        }
        let concrete = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "e2e-unknown-id"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for method in operations.as_object().expect("path item").keys() {
            let method =
                Method::from_bytes(method.to_ascii_uppercase().as_bytes()).expect("HTTP method");
            // This handler parses its body before authenticating, so only a
            // well-formed body reaches the credential check.
            let body = (method == Method::PUT && concrete == "/api/logging")
                .then(|| json!({ "filter": "info" }));
            let response = app
                .send(method.clone(), &concrete, None, body.as_ref())
                .await;
            assert_eq!(
                (response.status, response.error_code()),
                (StatusCode::UNAUTHORIZED, Some("UNAUTHORIZED")),
                "{method} {concrete} must require a credential: {}",
                response.text
            );
            checked += 1;
        }
    }
    assert!(
        checked > 100,
        "the OpenAPI document lists only {checked} protected operations"
    );

    // Routes the OpenAPI document does not list.
    for (method, path) in [
        (Method::GET, "/api/stream-proxy?url=http://127.0.0.1:9/x"),
        (Method::POST, "/api/mcp"),
    ] {
        let response = app.send(method.clone(), path, None, Some(&json!({}))).await;
        response.assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    }

    // The public routes.
    app.send(Method::GET, "/api/health/live", None, None)
        .await
        .expect_status(StatusCode::OK);
    app.send(Method::GET, "/api/health/idle", None, None)
        .await
        .expect_status(StatusCode::OK);
    for path in ["/api/logging/archive", "/api/logging/download"] {
        let response = app
            .send(Method::GET, &format!("{path}?token=e2e-bogus"), None, None)
            .await;
        response.assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    }
    let login = app.login(ADMIN_USERNAME, "e2e-wrong-password-1").await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login.body, invalid_login_body());

    app.shutdown().await;
}

#[tokio::test]
async fn malformed_expired_and_forged_credentials_are_rejected() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let claims = decode_claims(&token);
    let now = chrono::Utc::now().timestamp();

    let reminted = mint(&claims, JWT_SECRET);
    let mut expired = claims.clone();
    expired["iat"] = json!(now - 7200);
    expired["exp"] = json!(now - 120);
    let mut without_sid = claims.clone();
    without_sid
        .as_object_mut()
        .expect("claims object")
        .remove("sid");
    let mut unknown_user = claims.clone();
    unknown_user["sub"] = json!("e2e-no-such-user");

    let mut duplicated = HeaderMap::new();
    for _ in 0..2 {
        duplicated.append(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header value"),
        );
    }

    let cases = [
        ("Basic scheme", authorization("Basic x")),
        ("empty bearer", authorization("Bearer ")),
        ("two Authorization headers", duplicated),
        (
            "expired more than a minute ago",
            authorization(&format!("Bearer {}", mint(&expired, JWT_SECRET))),
        ),
        (
            "wrong signing secret",
            authorization(&format!(
                "Bearer {}",
                mint(&claims, "e2e-some-other-secret-with-enough-entropy-000")
            )),
        ),
        (
            "no session id",
            authorization(&format!("Bearer {}", mint(&without_sid, JWT_SECRET))),
        ),
        (
            "unknown user",
            authorization(&format!("Bearer {}", mint(&unknown_user, JWT_SECRET))),
        ),
    ];
    // A middleware-protected route and a handler that authenticates itself.
    for path in ["/api/streamers", "/api/health"] {
        let control = send_with_headers(
            &app,
            Method::GET,
            path,
            authorization(&format!("Bearer {reminted}")),
            None,
        )
        .await;
        assert_eq!(
            control.status,
            StatusCode::OK,
            "re-minted control token on {path}"
        );
        for (case, headers) in &cases {
            let response = send_with_headers(&app, Method::GET, path, headers.clone(), None).await;
            assert_eq!(
                (response.status, response.error_code()),
                (StatusCode::UNAUTHORIZED, Some("UNAUTHORIZED")),
                "{case} on {path}: {}",
                response.text
            );
        }
    }

    app.shutdown().await;
}

#[tokio::test]
async fn forced_password_change_blocks_everything_except_remediation() {
    let app = TestApp::start().await;

    let login = app
        .login(ADMIN_USERNAME, SEEDED_ADMIN_PASSWORD)
        .await
        .expect_status(StatusCode::OK);
    let body = login.body.as_object().expect("login body");
    assert_eq!(body["must_change_password"], true);
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 3600);
    assert_eq!(body["refresh_expires_in"], 604_800);
    assert_eq!(body["roles"], json!(["admin", "user"]));
    assert!(body["access_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(
        body["refresh_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty())
    );
    let forced = tokens_from(&login.body);

    for path in [
        "/api/streamers",
        "/api/auth/sessions",
        "/api/auth/api-keys",
        "/api/health",
        "/api/health/ready",
        "/api/logging",
    ] {
        let response = app.get(path, &forced.access).await;
        response.assert_error(StatusCode::FORBIDDEN, "PASSWORD_CHANGE_REQUIRED");
    }
    let refused = app
        .refused_socket("/api/downloads/ws", Some(&forced.access))
        .await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "the downloads socket must refuse the upgrade"
    );

    // Refreshing keeps the flag, so the frontend keeps routing to the change form.
    let refreshed = app
        .refresh(&forced.refresh)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(refreshed.body["must_change_password"], true);
    let refreshed = tokens_from(&refreshed.body);

    app.post("/api/auth/logout-all", &refreshed.access, &json!({}))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        streamers_status(&app, &refreshed.access).await,
        StatusCode::UNAUTHORIZED
    );

    let second = app
        .login_tokens(ADMIN_USERNAME, SEEDED_ADMIN_PASSWORD)
        .await;
    app.post(
        "/api/auth/change-password",
        &second.access,
        &json!({
            "current_password": SEEDED_ADMIN_PASSWORD,
            "new_password": ADMIN_PASSWORD,
            "confirm_password": ADMIN_PASSWORD,
        }),
    )
    .await
    .expect_status(StatusCode::OK);

    app.shutdown().await;
}

#[tokio::test]
async fn change_password_validates_with_400_and_revokes_every_session() {
    let app = TestApp::start().await;
    let old = app.admin().await.clone();
    let api_key = app
        .create_api_key(&old.access, "survives password change", "full")
        .await
        .raw;

    let change = |current: &str, new: &str| json!({ "current_password": current, "new_password": new, "confirm_password": new });
    for (current, new, message) in [
        (
            "e2e-not-the-password-1",
            "e2e-fresh-Password-2",
            "Current password is incorrect",
        ),
        (
            ADMIN_PASSWORD,
            ADMIN_PASSWORD,
            "Weak password: New password must be different from current password",
        ),
        (
            ADMIN_PASSWORD,
            "abc123",
            "Weak password: Password must be at least 8 characters",
        ),
        (
            ADMIN_PASSWORD,
            "12345678",
            "Weak password: Password must contain at least one letter",
        ),
        (
            ADMIN_PASSWORD,
            "abcdefgh",
            "Weak password: Password must contain at least one number",
        ),
    ] {
        let response = app
            .post(
                "/api/auth/change-password",
                &old.access,
                &change(current, new),
            )
            .await;
        response.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
        assert_eq!(response.body["message"], message);
    }
    // Rejected attempts leave the session usable.
    assert_eq!(streamers_status(&app, &old.access).await, StatusCode::OK);

    let new_password = "e2e-fresh-Password-2";
    app.post(
        "/api/auth/change-password",
        &old.access,
        &change(ADMIN_PASSWORD, new_password),
    )
    .await
    .expect_status(StatusCode::OK);

    assert_eq!(
        streamers_status(&app, &old.access).await,
        StatusCode::UNAUTHORIZED
    );
    app.refresh(&old.refresh)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    let stale = app.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    assert_eq!(stale.body, invalid_login_body());

    let fresh = app
        .login(ADMIN_USERNAME, new_password)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(fresh.body["must_change_password"], false);
    let sessions = app
        .get("/api/auth/sessions", &tokens_from(&fresh.body).access)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        sessions.body.as_array().map(Vec::len),
        Some(1),
        "only the new login remains: {}",
        sessions.text
    );

    // API keys are separate credentials, revoked only through DELETE.
    assert_eq!(streamers_status(&app, &api_key).await, StatusCode::OK);

    app.shutdown().await;
}

#[tokio::test]
async fn failed_logins_are_generic_and_lock_the_account_after_five() {
    // A fresh app whose admin has never logged in, so the limiter starts empty.
    let app = TestApp::start().await;

    let unknown = app.login("e2e-nobody", "e2e-wrong-password-1").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.body, invalid_login_body());
    let too_long = app.login(&"u".repeat(129), "e2e-wrong-password-1").await;
    too_long.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    for _ in 0..4 {
        let wrong = app.login(ADMIN_USERNAME, "e2e-wrong-password-1").await;
        assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            wrong.body,
            invalid_login_body(),
            "same body as an unknown user"
        );
    }
    // A success inside the budget starts the account's count over.
    app.login(ADMIN_USERNAME, SEEDED_ADMIN_PASSWORD)
        .await
        .expect_status(StatusCode::OK);

    // Five more failures, spread over spellings that share one budget.
    for username in ["admin", "ADMIN", " admin ", "Admin", "admin"] {
        let wrong = app.login(username, "e2e-wrong-password-1").await;
        assert_eq!(
            wrong.status,
            StatusCode::UNAUTHORIZED,
            "{username:?} is still inside the budget: {}",
            wrong.text
        );
    }

    let throttled = app.login(ADMIN_USERNAME, SEEDED_ADMIN_PASSWORD).await;
    let retry_after = throttled
        .header(header::RETRY_AFTER)
        .and_then(|value| value.parse::<u64>().ok());
    throttled.assert_error(StatusCode::TOO_MANY_REQUESTS, "TOO_MANY_REQUESTS");
    assert!(
        retry_after.is_some_and(|secs| (1..=900).contains(&secs)),
        "Retry-After: {retry_after:?}"
    );
    assert!(
        !throttled.text.to_lowercase().contains("admin"),
        "the 429 must not echo the username: {}",
        throttled.text
    );

    // Another account keeps its own budget.
    let other = app.login("e2e-other-user", "e2e-wrong-password-1").await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);
    assert_eq!(other.body, invalid_login_body());

    app.shutdown().await;
}

#[tokio::test]
async fn disabling_an_account_cuts_off_sessions_keys_and_sockets() {
    let app = TestApp::start().await;
    let admin = app.admin().await.clone();
    let api_key = app
        .create_api_key(&admin.access, "disabled owner", "full")
        .await
        .raw;
    // Fill the key's cached state while the account is still active.
    assert_eq!(streamers_status(&app, &api_key).await, StatusCode::OK);
    let mut socket = app
        .connect_socket("/api/downloads/ws", Some(&admin.access))
        .await;

    let db = app.database().await;
    sqlx::query("UPDATE users SET is_active = 0 WHERE username = ?")
        .bind(ADMIN_USERNAME)
        .execute(&db)
        .await
        .expect("disable the admin account");
    db.close().await;

    app.get("/api/streamers", &admin.access)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    app.refresh(&admin.refresh)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    let wrong = app.login(ADMIN_USERNAME, "e2e-wrong-password-1").await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong.body,
        invalid_login_body(),
        "a wrong password must not reveal that the account exists"
    );
    app.login(ADMIN_USERNAME, ADMIN_PASSWORD)
        .await
        .assert_error(StatusCode::FORBIDDEN, "ACCOUNT_DISABLED");

    let key_rejected = eventually(WAIT, "the API key to be rejected", || {
        let app = &app;
        let api_key = &api_key;
        async move {
            let response = app.get("/api/streamers", api_key).await;
            (response.status != StatusCode::OK).then_some(response)
        }
    });
    let (key_response, ()) = tokio::join!(key_rejected, assert_closes(&mut socket));
    key_response.assert_error(StatusCode::FORBIDDEN, "ACCOUNT_DISABLED");

    app.shutdown().await;
}

/// A session row written before device info was bounded can hold any
/// length. The session list shows it capped, and a refresh copies the capped
/// value, not the stored one, into the successor.
#[tokio::test]
async fn device_info_stored_before_it_was_bounded_is_capped_when_listed_and_refreshed() {
    let app = TestApp::start().await;
    let tokens = app.admin().await.clone();
    let legacy = format!("{}LEGACY_SUFFIX", "界".repeat(300));
    let capped = "界".repeat(256);
    let pool = app.database().await;
    sqlx::query("UPDATE refresh_tokens SET device_info = ?")
        .bind(&legacy)
        .execute(&pool)
        .await
        .expect("store an unbounded device description");

    let listed = app
        .get("/api/auth/sessions", &tokens.access)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        listed.body[0]["device_info"],
        capped.as_str(),
        "{}",
        listed.text
    );

    let refreshed = tokens_from(
        &app.refresh(&tokens.refresh)
            .await
            .expect_status(StatusCode::OK)
            .body,
    );
    let successor: Vec<Option<String>> =
        sqlx::query_scalar("SELECT device_info FROM refresh_tokens WHERE revoked_at IS NULL")
            .fetch_all(&pool)
            .await
            .expect("read the successor token");
    assert_eq!(successor, [Some(capped.clone())]);
    let listed = app
        .get("/api/auth/sessions", &refreshed.access)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        listed.body.as_array().map(Vec::len),
        Some(1),
        "{}",
        listed.text
    );
    assert_eq!(listed.body[0]["device_info"], capped.as_str());
    pool.close().await;

    app.shutdown().await;
}

#[tokio::test]
async fn refresh_rotates_tokens_and_replaying_a_rotated_token_revokes_every_session() {
    let app = TestApp::start().await;
    let a = app.admin().await.clone();
    let device_info = "设".repeat(300);
    let b = app
        .send(
            Method::POST,
            "/api/auth/login",
            None,
            Some(&json!({
                "username": ADMIN_USERNAME,
                "password": ADMIN_PASSWORD,
                "device_info": device_info,
            })),
        )
        .await
        .expect_status(StatusCode::OK);
    let b = tokens_from(&b.body);

    let sessions = app
        .get("/api/auth/sessions", &a.access)
        .await
        .expect_status(StatusCode::OK);
    let sessions = sessions.body.as_array().expect("session list").clone();
    assert_eq!(sessions.len(), 2, "one entry per login: {sessions:?}");
    let week_ms = 7 * 24 * 3600 * 1000;
    for session in &sessions {
        assert!(session["id"].as_str().is_some_and(|id| !id.is_empty()));
        let created_at = session["created_at"].as_i64().expect("created_at");
        let expires_at = session["expires_at"].as_i64().expect("expires_at");
        assert_recent(created_at);
        assert!(
            (created_at + week_ms - expires_at).abs() < 60_000,
            "expires in 7 days"
        );
    }
    // Newest first: B's device info is capped at 256 characters.
    assert_eq!(sessions[0]["device_info"], "设".repeat(256));
    assert_eq!(sessions[1]["device_info"], Value::Null);
    // The list caps again on read, so only the row shows the cap on write.
    let pool = app.database().await;
    let stored: Vec<Option<String>> =
        sqlx::query_scalar("SELECT device_info FROM refresh_tokens WHERE device_info IS NOT NULL")
            .fetch_all(&pool)
            .await
            .expect("read the stored device info");
    pool.close().await;
    assert_eq!(stored, [Some("设".repeat(256))]);

    // `tokenRefresh.ts` treats 400, 401 and 422 as terminal.
    let missing = app
        .send(Method::POST, "/api/auth/refresh", None, Some(&json!({})))
        .await;
    assert_eq!(missing.status, StatusCode::UNPROCESSABLE_ENTITY);
    app.refresh("")
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    app.refresh(&"0".repeat(64))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");

    let rotated = app.refresh(&a.refresh).await.expect_status(StatusCode::OK);
    assert_eq!(rotated.body["token_type"], "Bearer");
    assert_eq!(rotated.body["expires_in"], 3600);
    assert_eq!(rotated.body["refresh_expires_in"], 604_800);
    assert_eq!(rotated.body["must_change_password"], false);
    let a2 = tokens_from(&rotated.body);
    // The access token is not compared: one issued in the same second for
    // the same session is byte-identical.
    assert_ne!(a2.refresh, a.refresh);
    assert_eq!(streamers_status(&app, &a2.access).await, StatusCode::OK);
    let sessions = app
        .get("/api/auth/sessions", &a2.access)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        sessions.body.as_array().map(Vec::len),
        Some(2),
        "rotation replaces the session's token: {}",
        sessions.text
    );

    // Replaying the rotated token is treated as theft: every session ends.
    app.refresh(&a.refresh)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    for (what, token) in [("A' access", &a2.access), ("B access", &b.access)] {
        assert_eq!(
            streamers_status(&app, token).await,
            StatusCode::UNAUTHORIZED,
            "{what}"
        );
    }
    for (what, token) in [("A' refresh", &a2.refresh), ("B refresh", &b.refresh)] {
        assert_eq!(
            app.refresh(token).await.status,
            StatusCode::UNAUTHORIZED,
            "{what}"
        );
    }

    app.shutdown().await;
}

#[tokio::test]
async fn logout_ends_one_lineage_and_logout_all_ends_every_browser_session() {
    let app = TestApp::start().await;
    let a = app.admin().await.clone();
    let b = app.login_tokens(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    let api_key = app
        .create_api_key(&b.access, "survives logout-all", "full")
        .await
        .raw;
    let a2 = tokens_from(
        &app.refresh(&a.refresh)
            .await
            .expect_status(StatusCode::OK)
            .body,
    );

    // `logoutFn` sends only the refresh token.
    logout(&app, HeaderMap::new(), &a2.refresh)
        .await
        .expect_status(StatusCode::OK);
    for (what, token) in [
        ("original access", &a.access),
        ("rotated access", &a2.access),
    ] {
        assert_eq!(
            streamers_status(&app, token).await,
            StatusCode::UNAUTHORIZED,
            "{what}"
        );
    }
    assert_eq!(
        app.refresh(&a2.refresh).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(streamers_status(&app, &b.access).await, StatusCode::OK);

    // Rejected logouts leave B's session alone.
    for (what, headers, token) in [
        ("unknown token", HeaderMap::new(), "0".repeat(64)),
        (
            "Basic credential",
            authorization("Basic x"),
            b.refresh.clone(),
        ),
        (
            "API key credential",
            authorization(&format!("Bearer {api_key}")),
            b.refresh.clone(),
        ),
    ] {
        let response = logout(&app, headers, &token).await;
        response.assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
        assert_eq!(
            streamers_status(&app, &b.access).await,
            StatusCode::OK,
            "{what}"
        );
    }

    let response = app.post("/api/auth/logout-all", &api_key, &json!({})).await;
    response.assert_error(StatusCode::FORBIDDEN, "API_KEY_NOT_ALLOWED");
    assert_eq!(streamers_status(&app, &b.access).await, StatusCode::OK);

    app.post("/api/auth/logout-all", &b.access, &json!({}))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        streamers_status(&app, &b.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.refresh(&b.refresh).await.status,
        StatusCode::UNAUTHORIZED
    );
    // Logout-all ends browser sessions; API keys stay valid until revoked.
    assert_eq!(streamers_status(&app, &api_key).await, StatusCode::OK);

    app.shutdown().await;
}

#[tokio::test]
async fn api_keys_are_created_listed_used_and_revoked() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let empty = app
        .get("/api/auth/api-keys", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(empty.body, json!([]));

    let created = app
        .post(
            "/api/auth/api-keys",
            &token,
            &json!({ "name": "  Read key  ", "access_level": "read_only" }),
        )
        .await
        .expect_status(StatusCode::OK);
    let read_key = created.body["api_key"]
        .as_str()
        .expect("raw key")
        .to_string();
    assert!(
        read_key.len() == 69
            && read_key.starts_with("srec_")
            && read_key[5..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "raw key format: {read_key}"
    );
    let read_meta = &created.body["key"];
    assert_eq!(read_meta["name"], "Read key");
    assert_eq!(read_meta["key_prefix"], read_key[..13]);
    assert_eq!(read_meta["access_level"], "read_only");
    assert_eq!(read_meta["expires_at"], Value::Null);
    assert_eq!(read_meta["last_used_at"], Value::Null);
    assert_eq!(read_meta["revoked_at"], Value::Null);
    let read_created_at = read_meta["created_at"].as_i64().expect("created_at in ms");
    assert_recent(read_created_at);
    let read_id = read_meta["id"].as_str().expect("key id").to_string();

    // The list is ordered by creation time in milliseconds.
    eventually(WAIT, "the clock to pass the first key", || async {
        (now_ms() > read_created_at).then_some(())
    })
    .await;
    let expires_at = now_ms() + 3_600_000;
    let created = app
        .post(
            "/api/auth/api-keys",
            &token,
            &json!({ "name": "Full key", "access_level": "full", "expires_at": expires_at }),
        )
        .await
        .expect_status(StatusCode::OK);
    let full_key = created.body["api_key"]
        .as_str()
        .expect("raw key")
        .to_string();
    assert_eq!(created.body["key"]["expires_at"], expires_at);
    assert_eq!(created.body["key"]["access_level"], "full");

    app.get("/api/sessions", &read_key)
        .await
        .expect_status(StatusCode::OK);

    let listed = app
        .get("/api/auth/api-keys", &token)
        .await
        .expect_status(StatusCode::OK);
    assert!(
        !listed.text.contains(&read_key[13..]) && !listed.text.contains(&full_key[13..]),
        "the list must never contain a raw key"
    );
    let keys = listed.body.as_array().expect("key list");
    let names: Vec<_> = keys.iter().map(|key| key["name"].clone()).collect();
    assert_eq!(
        names,
        vec![json!("Full key"), json!("Read key")],
        "newest first"
    );
    let used_at = keys[1]["last_used_at"]
        .as_i64()
        .expect("last_used_at after use");
    assert_recent(used_at);
    assert_eq!(keys[0]["last_used_at"], Value::Null);

    app.delete(&format!("/api/auth/api-keys/{read_id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    // No cache window: the very next request fails.
    app.get("/api/sessions", &read_key)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    assert_eq!(streamers_status(&app, &full_key).await, StatusCode::OK);

    let listed = app
        .get("/api/auth/api-keys", &token)
        .await
        .expect_status(StatusCode::OK);
    let keys = listed.body.as_array().expect("key list");
    assert_eq!(keys.len(), 2, "a revoked key stays listed");
    let revoked_at = keys[1]["revoked_at"].as_i64().expect("revoked_at set");
    assert_recent(revoked_at);
    assert_eq!(keys[0]["revoked_at"], Value::Null);

    for id in [read_id.as_str(), "e2e-unknown-key"] {
        app.delete(&format!("/api/auth/api-keys/{id}"), &token)
            .await
            .assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    app.shutdown().await;
}

#[tokio::test]
async fn api_key_input_is_validated_and_expired_or_unknown_keys_fail() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    for body in [
        json!({ "name": "   ", "access_level": "full" }),
        json!({ "name": "a".repeat(101), "access_level": "full" }),
        json!({ "name": "past", "access_level": "full", "expires_at": now_ms() - 1000 }),
    ] {
        app.post("/api/auth/api-keys", &token, &body)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    }
    let listed = app
        .get("/api/auth/api-keys", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(listed.body, json!([]), "rejected input creates nothing");

    // 100 characters is the longest accepted name.
    app.post(
        "/api/auth/api-keys",
        &token,
        &json!({ "name": "a".repeat(100), "access_level": "read_only" }),
    )
    .await
    .expect_status(StatusCode::OK);

    app.get("/api/sessions", &format!("srec_{}", "0".repeat(64)))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");

    let created = app
        .post(
            "/api/auth/api-keys",
            &token,
            &json!({ "name": "short-lived", "access_level": "read_only", "expires_at": now_ms() + 5000 }),
        )
        .await
        .expect_status(StatusCode::OK);
    let short_lived = created.body["api_key"]
        .as_str()
        .expect("raw key")
        .to_string();
    app.get("/api/sessions", &short_lived)
        .await
        .expect_status(StatusCode::OK);
    let expired = eventually(WAIT, "the key to expire", || {
        let app = &app;
        let short_lived = &short_lived;
        async move {
            let response = app.get("/api/sessions", short_lived).await;
            (response.status != StatusCode::OK).then_some(response)
        }
    })
    .await;
    expired.assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");

    app.shutdown().await;
}

#[tokio::test]
async fn api_key_name_limit_counts_characters() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let name = "名".repeat(100);
    let created = app
        .post(
            "/api/auth/api-keys",
            &token,
            &json!({ "name": name, "access_level": "read_only" }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(created.body["key"]["name"], name);

    app.shutdown().await;
}

#[tokio::test]
async fn password_minimum_length_counts_characters() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let short = "密码密码1";
    let response = app
        .post(
            "/api/auth/change-password",
            &token,
            &json!({ "current_password": ADMIN_PASSWORD, "new_password": short }),
        )
        .await;
    response.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    assert_eq!(
        response.body["message"],
        "Weak password: Password must be at least 8 characters"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn read_only_keys_reach_only_the_allowlisted_routes() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let read_key = app
        .create_api_key(&token, "read only", "read_only")
        .await
        .raw;

    for path in [
        "/api/sessions",
        "/api/pipeline/stats",
        "/api/notifications/event-types",
        "/api/notifications/events",
        "/api/health",
    ] {
        let response = app.get(path, &read_key).await;
        assert_eq!(response.status, StatusCode::OK, "{path}: {}", response.text);
    }
    // Allowed through to the handler, which finds nothing.
    let segments = app
        .get("/api/sessions/e2e-unknown-session/segments", &read_key)
        .await;
    segments.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let media = app
        .send(
            Method::GET,
            &format!("/api/media/e2e-unknown-media/content?token={read_key}"),
            None,
            None,
        )
        .await;
    media.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    for (method, path) in [
        (Method::GET, "/api/streamers"),
        (Method::GET, "/api/config/global"),
        (Method::POST, "/api/sessions/batch-delete"),
        (Method::GET, "/api/auth/sessions"),
        (Method::GET, "/api/auth/api-keys"),
        (Method::POST, "/api/auth/api-keys"),
        (Method::POST, "/api/downloads/e2e-unknown-download/split"),
        (Method::GET, "/api/notifications/channels"),
    ] {
        let body = (method == Method::POST).then(|| json!({ "ids": ["e2e-unknown-session"] }));
        let response = app
            .send(method.clone(), path, Some(&read_key), body.as_ref())
            .await;
        assert_eq!(
            (response.status, response.error_code()),
            (StatusCode::FORBIDDEN, Some("API_KEY_READ_ONLY")),
            "{method} {path}: {}",
            response.text
        );
    }
    // MCP checks scope per tool, so the transport itself accepts the key.
    let mcp = app
        .client()
        .post(app.url("/api/mcp"))
        .bearer_auth(&read_key)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "e2e", "version": "0" }
            }
        }))
        .send()
        .await
        .expect("send the MCP initialize");
    let mcp = TestApp::read(mcp).await.expect_status(StatusCode::OK);
    assert!(mcp.body["result"]["serverInfo"].is_object(), "{}", mcp.text);

    app.shutdown().await;
}

#[tokio::test]
async fn full_keys_can_write_but_cannot_manage_keys_or_remediate() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let key = app.create_api_key(&token, "full", "full").await;
    let (full_key, key_id) = (key.raw, key.id);

    app.create_disabled_streamer(
        &full_key,
        json!({ "name": "e2e full key streamer", "url": "https://live.bilibili.com/990001" }),
    )
    .await;

    for (method, path, body) in [
        (Method::GET, "/api/auth/api-keys".to_string(), None),
        (
            Method::POST,
            "/api/auth/api-keys".to_string(),
            Some(json!({ "name": "minted by a key", "access_level": "full" })),
        ),
        (Method::DELETE, format!("/api/auth/api-keys/{key_id}"), None),
        (
            Method::POST,
            "/api/auth/change-password".to_string(),
            Some(
                json!({ "current_password": ADMIN_PASSWORD, "new_password": "e2e-key-Password-3" }),
            ),
        ),
        (
            Method::POST,
            "/api/auth/logout-all".to_string(),
            Some(json!({})),
        ),
    ] {
        let response = app
            .send(method.clone(), &path, Some(&full_key), body.as_ref())
            .await;
        assert_eq!(
            (response.status, response.error_code()),
            (StatusCode::FORBIDDEN, Some("API_KEY_NOT_ALLOWED")),
            "{method} {path}: {}",
            response.text
        );
    }
    // None of the refused calls had an effect.
    let keys = app
        .get("/api/auth/api-keys", &token)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(keys.body.as_array().map(Vec::len), Some(1));
    assert_eq!(keys.body[0]["revoked_at"], Value::Null);
    assert_eq!(streamers_status(&app, &token).await, StatusCode::OK);

    // A full key may list the owner's browser sessions.
    let sessions = app
        .get("/api/auth/sessions", &full_key)
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(sessions.body.as_array().map(Vec::len), Some(1));

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(&full_key).expect("header value"),
    );
    let response = send_with_headers(&app, Method::GET, "/api/streamers", headers, None).await;
    response.assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");

    app.shutdown().await;
}

#[tokio::test]
async fn cors_answers_any_origin_and_keeps_errors_readable() {
    let app = TestApp::start().await;
    let origin = "https://evil.example";

    let response = app
        .client()
        .get(app.url("/api/streamers"))
        .header(header::ORIGIN, origin)
        .send()
        .await
        .expect("send the request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let headers = response.headers().clone();
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("*")),
        "the frontend must be able to read a 401 to refresh"
    );
    let exposed = headers
        .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    for name in ["accept-ranges", "content-length", "content-range"] {
        assert!(exposed.contains(name), "{name} not exposed: {exposed}");
    }
    let body = TestApp::read(response).await;
    assert_eq!(body.error_code(), Some("UNAUTHORIZED"));

    let preflight = app
        .client()
        .request(Method::OPTIONS, app.url("/api/streamers"))
        .header(header::ORIGIN, origin)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type",
        )
        .send()
        .await
        .expect("send the preflight");
    assert_eq!(preflight.status(), StatusCode::OK);
    let headers = preflight.headers();
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("*"))
    );
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_METHODS),
        Some(&HeaderValue::from_static("*"))
    );
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_HEADERS),
        Some(&HeaderValue::from_static("*"))
    );

    app.shutdown().await;
}
