//! `/api/stream-proxy`: the SSRF target policy, the relay's header and
//! redirect handling, and the error codes the player reacts to.
//!
//! Every target is an IP literal or a `localhost` name the policy refuses
//! without a lookup, so nothing here depends on DNS or the public network.

use std::net::SocketAddr;

use axum::http::header;
use axum::response::{IntoResponse, Response as UpstreamResponse};
use reqwest::{Method, StatusCode};
use serde_json::{Map, Value, json};

use crate::support::{
    ClosedPort, LocalHttpServer, ReceivedRequest, Response, TestApp, id_of, with_query,
};

/// A public source URL for requests whose target checks are under test. It
/// only selects the outbound route and is never fetched.
const SOURCE: &str = "http://1.2.3.4/room";

const CREDENTIALS_REFUSED: &str = "URL credentials are not allowed";

const MANIFEST: &str = concat!(
    "#EXTM3U\n",
    "#EXT-X-VERSION:7\n",
    "#EXT-X-TARGETDURATION:2\n",
    "#EXT-X-MAP:URI=\"init.mp4\"\n",
    "#EXTINF:2.0,\n",
    "seg-1.ts\n",
    "#EXT-X-ENDLIST\n",
);

/// A loopback media origin for [`LocalHttpServer::start_with`]: an HLS
/// manifest and segment, a Range-aware media file that sets a cookie,
/// redirect chains, and redirects to targets the proxy must refuse.
fn upstream_reply(address: SocketAddr, request: &ReceivedRequest) -> UpstreamResponse {
    let path = request.path.split('?').next().unwrap_or_default();
    if let Some(remaining) = path.strip_prefix("/hops/") {
        let remaining: u32 = remaining.parse().expect("numeric hop count");
        if remaining == 0 {
            return "landed".into_response();
        }
        return (
            StatusCode::FOUND,
            [(header::LOCATION, format!("/hops/{}", remaining - 1))],
        )
            .into_response();
    }
    match path {
        "/live/index.m3u8" => (
            [
                (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
                (header::ETAG, "\"manifest-v1\""),
            ],
            MANIFEST,
        )
            .into_response(),
        "/live/seg-1.ts" => ([(header::CONTENT_TYPE, "video/mp2t")], "segment-one").into_response(),
        "/media" => {
            let common = [
                (header::CONTENT_TYPE, "video/mp2t"),
                (header::ACCEPT_RANGES, "bytes"),
                (header::SET_COOKIE, "tracking=upstream-secret"),
            ];
            if request.header("range").is_some() {
                (
                    StatusCode::PARTIAL_CONTENT,
                    common,
                    [(header::CONTENT_RANGE, "bytes 0-1/3")],
                    "ab",
                )
                    .into_response()
            } else {
                (common, "abc").into_response()
            }
        }
        "/to-file" => (
            StatusCode::FOUND,
            [(header::LOCATION, "file:///etc/passwd")],
        )
            .into_response(),
        "/to-credentials" => (
            StatusCode::FOUND,
            [(
                header::LOCATION,
                format!("http://user:secret@{address}/media"),
            )],
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The targets of the requests `upstream` received, in order.
fn paths(upstream: &LocalHttpServer) -> Vec<String> {
    upstream
        .received()
        .into_iter()
        .map(|request| request.path)
        .collect()
}

fn proxy_path(pairs: &[(&str, &str)]) -> String {
    with_query("/api/stream-proxy", pairs)
}

/// A proxy request authenticated only by its `token` query parameter, the way
/// a media element fetches it.
async fn fetch(app: &TestApp, path: &str) -> Response {
    app.send(Method::GET, path, None, None).await
}

const BAD_REQUEST: (StatusCode, &str) = (StatusCode::BAD_REQUEST, "BAD_REQUEST");
const BAD_GATEWAY: (StatusCode, &str) = (StatusCode::BAD_GATEWAY, "BAD_GATEWAY");

/// Fetch the proxy with the query `pairs` and assert the API error
/// `(status, code)` with `message`.
async fn assert_refused(
    app: &TestApp,
    pairs: &[(&str, &str)],
    (status, code): (StatusCode, &str),
    message: &str,
) {
    let response = fetch(app, &proxy_path(pairs)).await;
    assert_eq!(
        (
            response.status,
            response.error_code(),
            response.body["message"].as_str()
        ),
        (status, Some(code), Some(message)),
        "{pairs:?}: {}",
        response.text
    );
}

async fn allow_private_targets(app: &TestApp, token: &str, allow: bool) {
    let response = app
        .patch(
            "/api/config/global",
            token,
            &json!({ "stream_proxy_allow_private_targets": allow }),
        )
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(response.body["stream_proxy_allow_private_targets"], allow);
}

/// The `/api/stream-proxy` links a rewritten manifest contains, in order:
/// whole URI lines and quoted `URI="…"` attribute values.
fn manifest_links(manifest: &str) -> Vec<url::Url> {
    manifest
        .lines()
        .flat_map(|line| line.split('"'))
        .filter(|part| part.starts_with("/api/stream-proxy?"))
        .map(|link| url::Url::parse(&format!("http://backend{link}")).expect("valid proxy link"))
        .collect()
}

fn query_value(link: &url::Url, name: &str) -> Option<String> {
    link.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[tokio::test]
async fn proxy_refuses_dangerous_targets_and_relays_public_ones_by_default() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let not_allowed = "Target host is not allowed";
    let only_http = "Only http/https URLs are allowed";
    for (target, message) in [
        ("file:///etc/passwd", only_http),
        ("ftp://1.2.3.4/stream.flv", only_http),
        ("http://127.0.0.1/", not_allowed),
        // Alternative IPv4 spellings are normalized before the check.
        ("http://127.1:8080/", not_allowed),
        ("http://2130706433/", not_allowed),
        ("http://0.0.0.0/", not_allowed),
        ("http://10.0.0.1/", not_allowed),
        ("http://172.16.0.1/", not_allowed),
        ("http://192.168.1.1/", not_allowed),
        ("http://100.64.0.1/", not_allowed),
        ("http://169.254.169.254/latest/meta-data/", not_allowed),
        ("http://localhost:12555/api/config/global", not_allowed),
        ("http://x.localhost/", not_allowed),
        ("http://LOCALHOST./", not_allowed),
        ("http://[::1]/", not_allowed),
        ("http://[::]/", not_allowed),
        ("http://[fc00::1]/", not_allowed),
        ("http://[fd00::1]/", not_allowed),
        ("http://[fe80::1]/", not_allowed),
        ("http://[::ffff:127.0.0.1]/", not_allowed),
        ("http://[::ffff:10.0.0.1]/", not_allowed),
        (
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            not_allowed,
        ),
        ("http://[2001:db8::1]/", not_allowed),
        // 6to4 around 127.0.0.1 and 169.254.169.254; Teredo whose client
        // is 192.168.1.1, and whose server is 10.0.0.1.
        ("http://[2002:7f00:1::1]/", not_allowed),
        ("http://[2002:a9fe:a9fe::]/latest/meta-data/", not_allowed),
        (
            "http://[2001:0:4136:e378:8000:63bf:3f57:fefe]/",
            not_allowed,
        ),
        ("http://[2001:0:a00:1:8000:63bf:f7f7:f7f7]/", not_allowed),
        ("http://user:pass@1.2.3.4/", CREDENTIALS_REFUSED),
        ("not a url", "Invalid url parameter"),
    ] {
        // A valid source URL, so the target's own checks give the answer.
        let pairs = [("url", target), ("source_url", SOURCE), ("token", &token)];
        assert_refused(&app, &pairs, BAD_REQUEST, message).await;
    }

    // The source URL selects the outbound route and defaults to the target,
    // so it is held to the same scheme and credential rules.
    for (target, source) in [
        ("file:///etc/passwd", None),
        ("http://user:pass@1.2.3.4/", None),
        ("http://1.2.3.4/live.m3u8", Some("file:///etc/passwd")),
        (
            "http://1.2.3.4/live.m3u8",
            Some("http://user:pass@1.2.3.4/room"),
        ),
    ] {
        let mut pairs = vec![("url", target), ("token", token.as_str())];
        pairs.extend(source.map(|source| ("source_url", source)));
        assert_refused(&app, &pairs, BAD_REQUEST, "Invalid source URL").await;
    }
    assert_refused(&app, &[("token", &token)], BAD_REQUEST, "URL is required").await;

    // Authorization runs before any target handling.
    let refused_target = ("url", "file:///etc/passwd");
    fetch(&app, &proxy_path(&[refused_target]))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    let read_key = app
        .create_api_key(&token, "proxy-read_only", "read_only")
        .await
        .raw;
    fetch(&app, &proxy_path(&[refused_target, ("token", &read_key)]))
        .await
        .assert_error(StatusCode::FORBIDDEN, "API_KEY_READ_ONLY");
    let full_key = app.create_api_key(&token, "proxy-full", "full").await.raw;
    let pairs = [refused_target, ("token", &full_key)];
    assert_refused(&app, &pairs, BAD_REQUEST, "Invalid source URL").await;
    // An Authorization header is authoritative over the query fallback.
    let path = proxy_path(&[refused_target, ("token", &token)]);
    app.send(Method::GET, &path, Some("invalid"), None)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");

    // A public literal passes the check. The outbound route is a loopback
    // forward proxy, so the request never reaches the public network.
    let forward_proxy = LocalHttpServer::start(200, "relayed").await;
    let created = app
        .post(
            "/api/proxies",
            &token,
            &json!({ "name": "Loopback", "url": forward_proxy.url("") }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    app.patch(
        "/api/config/global",
        &token,
        &json!({ "proxy_route": { "kind": "proxy", "id": id_of(&created.body) } }),
    )
    .await
    .expect_status(StatusCode::OK);
    let public = "http://[2606:4700:4700::1111]/live.flv";
    let relayed = fetch(&app, &proxy_path(&[("url", public), ("token", &token)]))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(relayed.text, "relayed");
    assert_eq!(paths(&forward_proxy), [public]);

    app.shutdown().await;
}

#[tokio::test]
async fn allowing_private_targets_applies_to_the_next_request_and_manifests_stay_proxied() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let upstream = LocalHttpServer::start_with(upstream_reply).await;
    let manifest_url = upstream.url("/live/index.m3u8");
    let manifest = [("url", manifest_url.as_str()), ("token", &token)];

    assert_refused(&app, &manifest, BAD_REQUEST, "Target host is not allowed").await;
    assert!(paths(&upstream).is_empty(), "refused before connecting");

    allow_private_targets(&app, &token, true).await;
    let response = app
        .client()
        .get(app.url(&proxy_path(&manifest)))
        .send()
        .await
        .expect("fetch the manifest through the proxy");
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    // A rewritten manifest embeds the caller's token, so it must not be
    // revalidated against the upstream's validators.
    assert!(headers.get(header::ETAG).is_none());
    let rewritten = TestApp::read(response).await.text;
    assert!(
        rewritten.starts_with("#EXTM3U\n#EXT-X-VERSION:7\n"),
        "{rewritten}"
    );

    let links = manifest_links(&rewritten);
    let targets: Vec<_> = links.iter().map(|link| query_value(link, "url")).collect();
    assert_eq!(
        targets,
        [
            Some(upstream.url("/live/init.mp4")),
            Some(upstream.url("/live/seg-1.ts"))
        ],
        "{rewritten}"
    );
    for link in &links {
        assert_eq!(query_value(link, "token").as_deref(), Some(token.as_str()));
        assert_eq!(
            query_value(link, "source_url").as_deref(),
            Some(manifest_url.as_str())
        );
    }
    // The player follows a link as-is, with no Authorization header.
    let segment = fetch(&app, &links[1][url::Position::BeforePath..])
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(segment.text, "segment-one");
    assert_eq!(paths(&upstream), ["/live/index.m3u8", "/live/seg-1.ts"]);

    allow_private_targets(&app, &token, false).await;
    assert_refused(&app, &manifest, BAD_REQUEST, "Target host is not allowed").await;
    assert_eq!(paths(&upstream).len(), 2, "refused again without a restart");

    app.shutdown().await;
}

#[tokio::test]
async fn redirects_are_rechecked_at_every_hop_and_capped_at_five() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let upstream = LocalHttpServer::start_with(upstream_reply).await;
    allow_private_targets(&app, &token, true).await;
    let proxied = async |path: &str, expected: (StatusCode, &str), message: &str| {
        let url = upstream.url(path);
        assert_refused(&app, &[("url", &url), ("token", &token)], expected, message).await;
    };

    // Allowing private targets relaxes only the address check.
    let credentialed = format!("http://user:secret@{}/media", upstream.address());
    let pairs = [
        ("url", credentialed.as_str()),
        ("source_url", SOURCE),
        ("token", &token),
    ];
    assert_refused(&app, &pairs, BAD_REQUEST, CREDENTIALS_REFUSED).await;
    assert!(paths(&upstream).is_empty());

    let landing = upstream.url("/hops/5");
    let followed = fetch(&app, &proxy_path(&[("url", &landing), ("token", &token)]))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(followed.text, "landed");
    let hops = |from: u32, to: u32| (to..=from).rev().map(|n| format!("/hops/{n}"));
    assert_eq!(
        paths(&upstream),
        hops(5, 0).collect::<Vec<_>>(),
        "the request and five redirects"
    );

    proxied("/hops/6", BAD_GATEWAY, "Too many upstream redirects").await;
    assert_eq!(
        paths(&upstream)[6..],
        hops(6, 1).collect::<Vec<_>>(),
        "the sixth redirect is not followed"
    );

    proxied("/to-file", BAD_REQUEST, "Only http/https URLs are allowed").await;
    proxied("/to-credentials", BAD_REQUEST, CREDENTIALS_REFUSED).await;
    assert!(
        !paths(&upstream).contains(&"/media".to_string()),
        "a refused redirect target is never fetched"
    );

    let closed = ClosedPort::reserve();
    let unreachable = closed.url("/live.m3u8");
    let pairs = [("url", unreachable.as_str()), ("token", &token)];
    assert_refused(&app, &pairs, BAD_GATEWAY, "Proxy request failed").await;

    app.shutdown().await;
}

#[tokio::test]
async fn relay_forwards_only_safe_headers_ranges_and_hls_hints_and_rejects_bad_parameters() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let upstream = LocalHttpServer::start_with(upstream_reply).await;
    allow_private_targets(&app, &token, true).await;
    let media_url = upstream.url("/media");

    let custom = json!({
        "Referer": "https://live.example/room", "User-Agent": "TestPlayer/1.0", "Cookie": "cdn=1",
        "Host": "evil.example", "Connection": "close", "Keep-Alive": "timeout=5",
        "Proxy-Authorization": "Basic c2VjcmV0", "Transfer-Encoding": "chunked",
        "TE": "trailers", "Trailer": "Expires", "Upgrade": "websocket",
    })
    .to_string();
    let response = app
        .client()
        .get(app.url(&proxy_path(&[("url", &media_url), ("headers", &custom)])))
        .bearer_auth(&token)
        .header(header::RANGE, "bytes=0-1")
        .header(header::COOKIE, "srec_session=browser-secret")
        .header(header::ORIGIN, "http://player.example")
        .send()
        .await
        .expect("fetch media through the proxy");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let headers = response.headers().clone();
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 0-1/3");
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
    assert_eq!(headers[header::CONTENT_TYPE], "video/mp2t");
    assert!(headers.get(header::SET_COOKIE).is_none());
    let exposed = headers[header::ACCESS_CONTROL_EXPOSE_HEADERS]
        .to_str()
        .expect("ASCII header");
    assert!(
        exposed.to_ascii_lowercase().contains("content-range"),
        "{exposed}"
    );
    assert_eq!(TestApp::read(response).await.text, "ab");

    let requests = upstream.received();
    assert_eq!(requests.len(), 1);
    let received = &requests[0];
    let host = upstream.address().to_string();
    // Only the custom cookie: the caller's own cookie and bearer token stay
    // with the backend. Hop-by-hop and proxy headers are dropped.
    for (name, expected) in [
        ("referer", Some("https://live.example/room")),
        ("user-agent", Some("TestPlayer/1.0")),
        ("cookie", Some("cdn=1")),
        ("range", Some("bytes=0-1")),
        ("host", Some(host.as_str())),
        ("authorization", None),
        ("proxy-authorization", None),
        ("connection", None),
        ("keep-alive", None),
        ("transfer-encoding", None),
        ("te", None),
        ("trailer", None),
        ("upgrade", None),
    ] {
        assert_eq!(received.header(name), expected, "{name}");
    }

    let too_many: Map<String, Value> = (0..65)
        .map(|index| (format!("X-Header-{index}"), Value::from("1")))
        .collect();
    let too_many = Value::Object(too_many).to_string();
    let pairs = [
        ("url", media_url.as_str()),
        ("headers", &too_many),
        ("token", &token),
    ];
    assert_refused(&app, &pairs, BAD_REQUEST, "Too many custom headers").await;
    assert_eq!(paths(&upstream).len(), 1);

    // Low-latency HLS hints are accepted; they address the proxy, not the
    // origin.
    for skip in ["YES", "v2"] {
        let path = proxy_path(&[
            ("url", &media_url),
            ("token", &token),
            ("_HLS_msn", "7"),
            ("_HLS_part", "1"),
            ("_HLS_skip", skip),
        ]);
        let response = fetch(&app, &path).await.expect_status(StatusCode::OK);
        assert_eq!(response.text, "abc");
    }
    assert_eq!(paths(&upstream), ["/media", "/media", "/media"]);

    for invalid in [
        ("_HLS_skip", "foo"),
        ("_HLS_msn", "-1"),
        ("_HLS_part", "x"),
        ("stream", "-1"),
        ("unknown", "1"),
    ] {
        let path = proxy_path(&[("url", &media_url), ("token", &token), invalid]);
        assert_eq!(
            fetch(&app, &path).await.status,
            StatusCode::BAD_REQUEST,
            "{invalid:?}"
        );
    }
    assert_eq!(paths(&upstream).len(), 3);

    app.shutdown().await;
}

#[tokio::test]
async fn unknown_playback_handles_return_the_codes_the_player_renews_on() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let media = "http://127.0.0.1:9/live/seg-1.ts";
    let managed = [
        ("playback_handle", "unknown-handle"),
        ("stream", "0"),
        ("url", media),
        ("token", token.as_str()),
    ];

    fetch(&app, &proxy_path(&managed))
        .await
        .assert_error(StatusCode::GONE, "PLAYBACK_CONTEXT_EXPIRED");

    // A managed request carries the context's own headers and source only.
    for extra in [("headers", "{}"), ("source_url", "http://127.0.0.1/room")] {
        let pairs = [&managed[..], &[extra]].concat();
        let message = "Managed playback cannot contain client headers or a source URL";
        assert_refused(&app, &pairs, BAD_REQUEST, message).await;
    }
    // Each of the handle, the stream and the URL is required.
    for missing in 0..3 {
        let mut pairs = managed.to_vec();
        pairs.remove(missing);
        let message = "Managed playback requires a playback handle, stream and URL";
        assert_refused(&app, &pairs, BAD_REQUEST, message).await;
    }

    let renew = app
        .post(
            "/api/parse/playback/renew",
            &token,
            &json!({ "playback_handle": "unknown-handle" }),
        )
        .await;
    renew.assert_error(StatusCode::GONE, "PLAYBACK_CONTEXT_EXPIRED");
    let extra_field = app
        .post(
            "/api/parse/playback/renew",
            &token,
            &json!({ "playback_handle": "unknown-handle", "url": media }),
        )
        .await;
    assert_eq!(extra_field.status, StatusCode::UNPROCESSABLE_ENTITY);

    app.shutdown().await;
}
