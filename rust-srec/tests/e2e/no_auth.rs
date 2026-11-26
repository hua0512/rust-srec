//! A backend served without authentication, as `AUTH_DISABLED=true` on a
//! loopback address runs it: browser requests are limited by their `Origin`
//! and `Host` instead of a credential.

use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use serde_json::json;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::support::{Response, TestApp, WAIT};

/// First entry of the default `API_CORS_ORIGINS`: the frontend's dev server.
const LISTED: &str = "http://localhost:15275";
const FOREIGN: &str = "https://evil.example";
/// A name an attacker's DNS points at 127.0.0.1.
const REBOUND_HOST: &str = "recorder.evil.example";

/// A request browsers send without a preflight: a body-less `POST`, here
/// cancelling a job that does not exist.
const SIMPLE_POST: &str = "/api/pipeline/jobs/e2e-unknown-job/cancel";

fn headers(pairs: &[(HeaderName, &str)]) -> HeaderMap {
    pairs
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                HeaderValue::from_str(value).expect("valid header value"),
            )
        })
        .collect()
}

async fn send(app: &TestApp, method: Method, path: &str, headers: HeaderMap) -> Response {
    let request = app.client().request(method, app.url(path)).headers(headers);
    TestApp::read(request.send().await.expect("send the request")).await
}

/// The status of a WebSocket upgrade to the log stream with `origin`.
async fn upgrade_status(app: &TestApp, origin: &str) -> StatusCode {
    let mut request = app
        .url("/api/logging/stream")
        .replacen("http://", "ws://", 1)
        .into_client_request()
        .expect("valid WebSocket request");
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_str(origin).expect("valid origin"),
    );
    match tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
        .await
        .expect("WebSocket handshake timed out")
    {
        Ok((mut socket, response)) => {
            socket.close(None).await.ok();
            response.status()
        }
        Err(tungstenite::Error::Http(response)) => response.status(),
        Err(error) => panic!("WebSocket handshake failed: {error}"),
    }
}

/// Callers need no credential, MCP write tools included, while browser
/// requests are limited by their origin. Pins the production layer order of
/// `ApiServer::build_router`: the CORS layer answers preflights itself, and
/// the origin guard inside it refuses the requests browsers send without one.
#[tokio::test]
async fn a_server_without_auth_allows_mcp_writes_and_refuses_foreign_origins() {
    let app = TestApp::start_without_auth().await;
    let own_host = app.url("").trim_start_matches("http://").to_owned();
    let port = own_host
        .rsplit_once(':')
        .expect("host and port")
        .1
        .to_owned();

    // Callers need no credential, and callers that send no Origin (curl,
    // the frontend's server) pass the guard.
    send(&app, Method::GET, "/api/streamers", HeaderMap::new())
        .await
        .expect_status(StatusCode::OK);
    let passes = send(&app, Method::POST, SIMPLE_POST, HeaderMap::new()).await;
    passes.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let same_site = format!("http://{own_host}");
    let localhost = format!("localhost:{port}");
    let ipv6_loopback = format!("[::1]:{port}");
    for (case, pairs, refused) in [
        ("listed origin", vec![(header::ORIGIN, LISTED)], false),
        (
            "same-site origin",
            vec![(header::ORIGIN, same_site.as_str())],
            false,
        ),
        (
            "localhost host",
            vec![(header::HOST, localhost.as_str())],
            false,
        ),
        (
            "IPv6 loopback host",
            vec![(header::HOST, ipv6_loopback.as_str())],
            false,
        ),
        ("foreign origin", vec![(header::ORIGIN, FOREIGN)], true),
        ("opaque origin", vec![(header::ORIGIN, "null")], true),
        ("rebound host", vec![(header::HOST, REBOUND_HOST)], true),
        (
            "listed origin on a rebound host",
            vec![(header::HOST, REBOUND_HOST), (header::ORIGIN, LISTED)],
            true,
        ),
    ] {
        let response = send(&app, Method::POST, SIMPLE_POST, headers(&pairs)).await;
        let expected = if refused {
            (StatusCode::FORBIDDEN, "FORBIDDEN_ORIGIN")
        } else {
            (StatusCode::NOT_FOUND, "NOT_FOUND")
        };
        assert_eq!(
            (response.status, response.error_code()),
            (expected.0, Some(expected.1)),
            "{case}: {}",
            response.text
        );
    }

    // A WebSocket upgrade is not preflighted either.
    assert_eq!(
        upgrade_status(&app, FOREIGN).await,
        StatusCode::FORBIDDEN,
        "foreign origin upgrade"
    );
    assert_eq!(
        upgrade_status(&app, LISTED).await,
        StatusCode::SWITCHING_PROTOCOLS,
        "listed origin upgrade"
    );

    // Preflights are answered before the guard, and allow only listed
    // origins.
    for (origin, allowed) in [(FOREIGN, None), (LISTED, Some(LISTED))] {
        let preflight = send(
            &app,
            Method::OPTIONS,
            SIMPLE_POST,
            headers(&[
                (header::ORIGIN, origin),
                (header::ACCESS_CONTROL_REQUEST_METHOD, "POST"),
                (header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type"),
            ]),
        )
        .await;
        assert_eq!(preflight.status, StatusCode::OK, "{origin}");
        assert_eq!(
            preflight.header(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            allowed,
            "{origin}"
        );
    }

    // MCP write tools need no credential either.
    let downloads = async || {
        send(&app, Method::GET, "/api/config/global", HeaderMap::new())
            .await
            .ok()["max_concurrent_downloads"]
            .clone()
    };
    assert_ne!(downloads().await, 4);
    let response = app
        .client()
        .post(app.url("/api/mcp"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "config_update_global",
                "arguments": { "updates": { "max_concurrent_downloads": 4 } },
            },
        }))
        .send()
        .await
        .expect("send the MCP request");
    let updated = TestApp::read(response).await.ok();
    assert_eq!(updated["result"]["isError"], false, "{updated}");
    assert_eq!(downloads().await, 4);

    app.shutdown().await;
}
