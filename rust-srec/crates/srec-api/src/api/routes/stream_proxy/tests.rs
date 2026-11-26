use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request as HttpRequest, StatusCode, header};
use axum::routing::get;
use bytes::Bytes;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::hls::{ManagedRelay, RelayContext, rewrite_hls_manifest};
use super::managed::{managed_media_headers, managed_target};
use super::ssrf::{resolve_public_addresses, stream_proxy_client};
use super::upstream::{fetch_upstream_bound, is_page_or_api_content_type};
use super::*;
use crate::api::cors::CorsPolicy;
use crate::api::error::ApiError;
use crate::proxies::ProxyTarget;

fn build_query(pairs: &[(&str, &str)]) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.finish()
}

fn test_state(allow_private_targets: bool) -> StreamProxyState {
    StreamProxyState {
        auth_service: None,
        source_config: None,
        upstream_proxy: ProxyTarget::Direct,
        config_service: None,
        allow_private_targets,
        cors: CorsPolicy::AnyOrigin,
        cancel_token: CancellationToken::new(),
    }
}

fn managed_context(
    streams: &[&str],
) -> (
    crate::api::playback_context::PlaybackContextService,
    crate::api::playback_context::ManagedPlayback,
) {
    let service = crate::api::playback_context::PlaybackContextService::default();
    let (source, snapshot, mut media) = crate::api::playback_context::test_bundle(streams[0]);
    for url in &streams[1..] {
        let mut stream = media.streams[0].clone();
        stream.url = (*url).to_owned();
        media.streams.push(stream);
    }
    let context = service.insert("alice", source, snapshot, media).unwrap();
    (service, context)
}

fn proxy_links(manifest: &str) -> Vec<url::Url> {
    manifest
        .lines()
        .flat_map(|line| line.split('"'))
        .filter(|part| part.starts_with("/api/stream-proxy?"))
        .map(|part| url::Url::parse(&format!("http://backend.test{part}")).unwrap())
        .collect()
}

fn query_value(link: &url::Url, name: &str) -> Option<String> {
    link.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[test]
fn managed_relay_recognises_pages_and_api_answers_in_any_spelling() {
    let page_or_api =
        |value: &str| is_page_or_api_content_type(&HeaderValue::from_str(value).unwrap());
    for value in [
        "text/html",
        "Text/HTML; charset=utf-8",
        " text/html",
        "application/json",
        "application/problem+json",
        "video/mp4, text/html",
    ] {
        assert!(page_or_api(value), "{value}");
    }
    for value in [
        "video/mp2t",
        "application/vnd.apple.mpegurl",
        "text/plain",
        "application/octet-stream",
    ] {
        assert!(!page_or_api(value), "{value}");
    }
}

#[test]
fn managed_manifest_links_carry_real_urls_and_the_handle_only_for_allowed_origins() {
    let (service, context) = managed_context(&["https://cdn.test/live/master.m3u8"]);
    let data = service.get(&context.handle, "alice").unwrap();
    let manifest = concat!(
        "#EXTM3U\n",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.test/key?sig=k\"\n",
        "#EXT-X-MAP:URI=\"init.mp4?sig=i\"\n",
        "#EXTINF:2,\n",
        "segment.ts?sig=s\n",
        "#EXTINF:2,\n",
        "https://edge.test/segment.ts\n",
    );
    let rewritten = rewrite_hls_manifest(
        manifest,
        &url::Url::parse("https://cdn.test/live/index.m3u8").unwrap(),
        RelayContext {
            token: Some("api-token"),
            source_url: Some(&data.source.url),
            playback: Some(ManagedRelay {
                handle: &context.handle,
                stream: 0,
                data: &data,
            }),
            ..RelayContext::default()
        },
    );
    for secret in ["secret-cookie", "secret-access", "Cookie", "Authorization"] {
        assert!(!rewritten.contains(secret), "{secret} leaked");
    }
    assert!(!rewritten.contains("headers="));
    let links = proxy_links(&rewritten);
    assert_eq!(links.len(), 4);
    let by_target = |target: &str| {
        links
            .iter()
            .find(|link| query_value(link, "url").as_deref() == Some(target))
            .unwrap_or_else(|| panic!("no link for {target}"))
    };
    for target in [
        "https://cdn.test/live/init.mp4?sig=i",
        "https://cdn.test/live/segment.ts?sig=s",
    ] {
        let link = by_target(target);
        assert_eq!(
            query_value(link, "playback_handle").as_deref(),
            Some(context.handle.as_str())
        );
        assert_eq!(query_value(link, "stream").as_deref(), Some("0"));
    }
    for target in [
        "https://keys.test/key?sig=k",
        "https://edge.test/segment.ts",
    ] {
        let link = by_target(target);
        assert!(query_value(link, "playback_handle").is_none());
        assert!(query_value(link, "stream").is_none());
        assert_eq!(
            query_value(link, "source_url").as_deref(),
            Some("https://platform.test/room")
        );
    }
}

#[test]
fn managed_handle_is_refused_for_hosts_outside_its_stream_and_source() {
    let (service, context) =
        managed_context(&["https://cdn.test/a.m3u8", "https://other-cdn.test/b.m3u8"]);
    let data = service.get(&context.handle, "alice").unwrap();
    assert!(managed_target(&data, 0, "https://cdn.test/segment.ts").is_ok());
    assert!(managed_target(&data, 0, "https://platform.test/key").is_ok());
    assert!(managed_target(&data, 1, "https://other-cdn.test/segment.ts").is_ok());
    for (stream, url) in [
        (0, "https://attacker.test/collect"),
        (0, "https://other-cdn.test/segment.ts"),
        (0, "http://cdn.test/segment.ts"),
        (0, "https://cdn.test.attacker.test/"),
    ] {
        let error = ApiError::from(managed_target(&data, stream, url).unwrap_err());
        assert_eq!(error.status, StatusCode::FORBIDDEN, "{url}");
        assert_eq!(error.code, "PLAYBACK_HOST_NOT_ALLOWED");
    }
    for (stream, url) in [
        (2, "https://cdn.test/segment.ts"),
        (0, "ftp://cdn.test/segment.ts"),
        (0, "https://user:pass@cdn.test/segment.ts"),
    ] {
        assert!(matches!(
            managed_target(&data, stream, url),
            Err(crate::api::playback_context::PlaybackError::InvalidResource)
        ));
    }
}

#[test]
fn managed_resource_headers_keep_each_cdns_cookie_without_changing_the_account() {
    let service = crate::api::playback_context::PlaybackContextService::default();
    let (source, snapshot, mut media) =
        crate::api::playback_context::test_bundle("https://cdn.test/a.m3u8");
    let mut other = media.streams[0].clone();
    media.streams[0].extras = Some(
        serde_json::json!({"headers":{"Cookie":"session=stale; edge=cdn-a","Authorization":"Bearer cdn-a"}}),
    );
    other.extras = Some(
        serde_json::json!({"headers":{"Cookie":"session=stale; edge=cdn-b","Authorization":"Bearer cdn-b"}}),
    );
    media.streams.push(other);
    let context = service.insert("alice", source, snapshot, media).unwrap();
    let data = service.get(&context.handle, "alice").unwrap();
    let a = managed_media_headers(&data, 0, &HeaderMap::new()).unwrap();
    let b = managed_media_headers(&data, 1, &HeaderMap::new()).unwrap();
    assert!(a[header::COOKIE].to_str().unwrap().contains("edge=cdn-a"));
    assert!(b[header::COOKIE].to_str().unwrap().contains("edge=cdn-b"));
    assert!(
        a[header::COOKIE]
            .to_str()
            .unwrap()
            .contains("session=secret-cookie")
    );
    assert!(
        b[header::COOKIE]
            .to_str()
            .unwrap()
            .contains("session=secret-cookie")
    );
    assert_eq!(a[header::AUTHORIZATION], "Bearer cdn-a");
    assert_eq!(b[header::AUTHORIZATION], "Bearer cdn-b");
    assert_eq!(data.snapshot.material.cookies, "session=secret-cookie");
}

#[tokio::test]
async fn managed_redirect_cannot_forward_authentication_to_another_variant_origin() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination_url = format!("http://{}/segment", destination.local_addr().unwrap());
        let other_variant_url = destination_url.clone();
        let destination_app = Router::new().route(
            "/segment",
            get(|headers: HeaderMap| async move {
                assert!(headers.get(header::COOKIE).is_none());
                assert!(headers.get(header::AUTHORIZATION).is_none());
                assert!(headers.get("x-provider-token").is_none());
                "segment"
            }),
        );
        let destination_task = tokio::spawn(async move {
            axum::serve(destination, destination_app).await.unwrap();
        });
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_url = format!("http://{}/master", source.local_addr().unwrap());
        let source_app = Router::new().route(
            "/master",
            get(move |headers: HeaderMap| {
                let location = destination_url.clone();
                async move {
                    assert_eq!(
                        headers.get(header::COOKIE).unwrap(),
                        "session=secret-cookie"
                    );
                    (StatusCode::FOUND, [(header::LOCATION, location)])
                }
            }),
        );
        let source_task = tokio::spawn(async move {
            axum::serve(source, source_app).await.unwrap();
        });
        let service = crate::api::playback_context::PlaybackContextService::default();
        let (source, snapshot, mut media) = crate::api::playback_context::test_bundle(&source_url);
        let mut other_variant = media.streams[0].clone();
        other_variant.url = other_variant_url;
        media.streams.push(other_variant);
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let data = service.get(&context.handle, "alice").unwrap();
        let client = stream_proxy_client(true, &ProxyTarget::Direct).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("session=secret-cookie"),
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        headers.insert("x-provider-token", HeaderValue::from_static("secret"));
        let response = fetch_upstream_bound(
            &client,
            url::Url::parse(&source_url).unwrap(),
            &headers,
            true,
            Some((&data, 0)),
        )
        .await
        .unwrap();
        assert_eq!(response.text().await.unwrap(), "segment");
        source_task.abort();
        destination_task.abort();
    })
    .await
    .expect("redirect fixture must finish");
}

#[test]
fn rewrites_hls_resource_uris_without_changing_line_endings() {
    let manifest = concat!(
        "#EXTM3U\r\n",
        "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio/index.m3u8\"\r\n",
        "#EXT-X-STREAM-INF:BANDWIDTH=1000\r\n",
        "video/index.m3u8\r\n",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"../key.bin\"\r\n",
        "#EXT-X-MAP:URI = \"init.mp4\"\r\n",
        "#EXT-X-PART:DURATION=0.333,URI=\"part.ts\"\r\n",
        "segment.ts"
    );
    let base_url = url::Url::parse("https://media.example/live/master.m3u8").unwrap();
    let rewritten = rewrite_hls_manifest(
        manifest,
        &base_url,
        RelayContext {
            headers: Some(r#"{"Referer":"https://source.example/"}"#),
            token: Some("desktop-token"),
            source_url: Some("https://source.example/channel"),
            web: false,
            playback: None,
        },
    );

    assert_eq!(rewritten.matches("/api/stream-proxy?").count(), 6);
    assert!(rewritten.contains("url=https%3A%2F%2Fmedia.example%2Flive%2Faudio%2Findex.m3u8"));
    assert!(rewritten.contains("url=https%3A%2F%2Fmedia.example%2Fkey.bin"));
    assert!(
        rewritten.contains("headers=%7B%22Referer%22%3A%22https%3A%2F%2Fsource.example%2F%22%7D")
    );
    assert!(rewritten.contains("token=desktop-token"));
    assert_eq!(rewritten.matches("\r\n").count(), 7);
    assert!(!rewritten.ends_with('\n'));
}

#[tokio::test]
async fn public_address_resolver_rejects_private_hosts() {
    assert!(resolve_public_addresses("localhost").await.is_err());
}

#[test]
fn invalid_upstream_proxy_fails_without_falling_back_to_direct() {
    let invalid =
        ProxyTarget::Explicit(crate::proxies::ProxyEndpoint::new("http://[invalid", None));
    let error = stream_proxy_client(false, &invalid).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn shutdown_cancellation_terminates_infinite_relayed_stream_and_sets_private_cache_control() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let app = Router::new().route(
            "/live.flv",
            get(|| async {
                let first = futures::stream::once(async {
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(b"FLV-initial-segment-bytes"))
                });
                let never = futures::stream::pending::<Result<Bytes, std::io::Error>>();
                (
                    [
                        (header::CONTENT_TYPE, "video/x-flv"),
                        (header::CACHE_CONTROL, "public, max-age=3600"),
                    ],
                    Body::from_stream(futures::StreamExt::chain(first, never)),
                )
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });

    let state = test_state(true);
    let cancel_token = state.cancel_token.clone();
    let app = Router::new()
        .nest("/api/stream-proxy", super::router::<StreamProxyState>())
        .with_state(state);
    let target = format!("http://{addr}/live.flv");
    let query = build_query(&[("url", &target), ("token", "viewer-bearer-token")]);
    let request = HttpRequest::builder()
        .uri(format!("/api/stream-proxy?{query}"))
        .body(Body::empty())
        .unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), app.oneshot(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );

    cancel_token.cancel();
    let collected = tokio::time::timeout(
        Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .expect("cancelled stream must finish promptly")
    .unwrap();
    assert_eq!(&collected[..], b"FLV-initial-segment-bytes");

    upstream_task.abort();
}
