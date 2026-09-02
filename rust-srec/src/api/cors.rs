//! Cross-origin policy shared by the router's `CorsLayer` and by handlers
//! that write their own CORS headers (`routes::stream_proxy`).

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, header};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::{info, warn};

/// Comma-separated list of exact origins (`scheme://host[:port]`) allowed to
/// call the API from a browser while authentication is disabled.
pub const CORS_ORIGINS_ENV: &str = "API_CORS_ORIGINS";

/// Origins allowed when `API_CORS_ORIGINS` is unset and the API runs without
/// an `AuthService`: the Vite dev server (`frontend/package.json` runs it on
/// 15275) and the origins a Tauri v2 webview loads from.
pub const DEFAULT_UNAUTHENTICATED_ORIGINS: [&str; 4] = [
    "http://localhost:15275",
    "http://127.0.0.1:15275",
    "tauri://localhost",
    "http://tauri.localhost",
];

/// Which origins may make cross-origin browser requests.
#[derive(Clone, Debug)]
pub enum CorsPolicy {
    /// Every origin, answered with `Access-Control-Allow-Origin: *`.
    ///
    /// Used while `AppState::auth_service` is set: every route behind
    /// `AuthLayer` needs a bearer token that a third-party page cannot obtain
    /// from the browser, and no route reads cookies, so a permissive origin
    /// list grants a foreign page nothing.
    AnyOrigin,
    /// Only these exact origins, compared byte-for-byte against the request's
    /// `Origin` header.
    ///
    /// Used while `AppState::auth_service` is `None`, where any origin that
    /// passes this check can drive every route — including
    /// `routes::pipeline` job submission — with no credential at all.
    Exact(Arc<[HeaderValue]>),
}

impl CorsPolicy {
    /// Build the router-wide layer. Callers add the method/header/expose
    /// rules; only the origin rule differs between variants.
    pub fn layer(&self) -> CorsLayer {
        let layer = CorsLayer::new();
        match self {
            Self::AnyOrigin => layer.allow_origin(Any),
            Self::Exact(origins) => layer.allow_origin(AllowOrigin::list(origins.iter().cloned())),
        }
    }

    /// Value for `Access-Control-Allow-Origin`, or `None` when `origin` is
    /// not allowed and the header must be omitted entirely.
    pub fn allow_origin_value(&self, origin: Option<&HeaderValue>) -> Option<HeaderValue> {
        match self {
            Self::AnyOrigin => Some(HeaderValue::from_static("*")),
            Self::Exact(origins) => {
                let origin = origin?;
                origins.iter().find(|allowed| *allowed == origin).cloned()
            }
        }
    }

    /// Write `Access-Control-Allow-Origin` for a request that arrived with
    /// `origin`, leaving `headers` untouched when the origin is not allowed.
    ///
    /// `Vary: Origin` is appended whenever the value depends on the request,
    /// so a shared cache cannot serve one origin's response to another.
    pub fn apply_allow_origin(&self, origin: Option<&HeaderValue>, headers: &mut HeaderMap) {
        if matches!(self, Self::Exact(_)) {
            headers.append(header::VARY, HeaderValue::from_static("origin"));
        }
        if let Some(value) = self.allow_origin_value(origin) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        }
    }
}

/// Pick the policy for an API instance.
///
/// `auth_enabled` mirrors `AppState::auth_service.is_some()`: it is the only
/// thing standing between a foreign origin and every mutating route, so it is
/// also the only input that unlocks [`CorsPolicy::AnyOrigin`].
pub fn policy_for(auth_enabled: bool, allowed_origins: &Arc<[HeaderValue]>) -> CorsPolicy {
    if auth_enabled {
        CorsPolicy::AnyOrigin
    } else {
        CorsPolicy::Exact(allowed_origins.clone())
    }
}

/// Read the allowlist used by [`CorsPolicy::Exact`] from `API_CORS_ORIGINS`,
/// falling back to [`DEFAULT_UNAUTHENTICATED_ORIGINS`] when the variable is
/// unset or blank.
///
/// Entries that are not a bare `scheme://host[:port]` are dropped with a
/// warning rather than failing startup; an explicitly configured list that
/// leaves no usable entry denies all cross-origin access.
pub fn allowed_origins_from_env() -> Arc<[HeaderValue]> {
    let configured = std::env::var(CORS_ORIGINS_ENV).ok();
    let configured = configured
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty());

    let Some(raw) = configured else {
        return parse_origins(DEFAULT_UNAUTHENTICATED_ORIGINS.iter().copied());
    };

    let origins = parse_origins(raw.split(','));
    if origins.is_empty() {
        warn!(
            env = CORS_ORIGINS_ENV,
            "No usable origin in {CORS_ORIGINS_ENV}; cross-origin browser access is denied while authentication is disabled"
        );
    }
    origins
}

/// Normalise and validate each entry, discarding the rejected ones.
fn parse_origins<'a>(entries: impl Iterator<Item = &'a str>) -> Arc<[HeaderValue]> {
    let mut origins: Vec<HeaderValue> = Vec::new();
    for entry in entries {
        if entry.trim().is_empty() {
            continue;
        }
        match normalize_origin(entry) {
            Ok(origin) => {
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
            Err(reason) => warn!(
                env = CORS_ORIGINS_ENV,
                entry = %entry.trim(),
                reason,
                "Ignoring malformed CORS origin"
            ),
        }
    }
    origins.into()
}

/// Turn one configured entry into the exact bytes a browser sends in
/// `Origin`: lowercase, no trailing slash, no path.
fn normalize_origin(entry: &str) -> Result<HeaderValue, &'static str> {
    let value = entry.trim().trim_end_matches('/').to_ascii_lowercase();
    if value == "*" {
        return Err("a wildcard cannot be combined with an explicit allowlist");
    }
    let Some((scheme, authority)) = value.split_once("://") else {
        return Err("expected scheme://host[:port]");
    };
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Err("invalid scheme");
    }
    if authority.is_empty() || authority.contains(['/', '?', '#']) {
        return Err("expected scheme://host[:port] without a path");
    }
    if value.chars().any(char::is_whitespace) {
        return Err("origins cannot contain whitespace");
    }
    HeaderValue::from_str(&value).map_err(|_| "not a valid header value")
}

/// Log the resolved policy once, at router construction.
pub fn log_policy(policy: &CorsPolicy) {
    match policy {
        CorsPolicy::AnyOrigin => {
            info!("CORS: any origin (bearer authentication is enforced on protected routes)")
        }
        CorsPolicy::Exact(origins) => {
            let origins = origins
                .iter()
                .filter_map(|origin| origin.to_str().ok())
                .collect::<Vec<_>>()
                .join(", ");
            info!(
                allowed_origins = %origins,
                "CORS: restricted to an explicit allowlist because authentication is disabled; override with {CORS_ORIGINS_ENV}"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("test origin should be a valid header value")
    }

    fn exact(values: &[&str]) -> CorsPolicy {
        CorsPolicy::Exact(parse_origins(values.iter().copied()))
    }

    #[test]
    fn defaults_cover_the_dev_server_and_desktop_webview() {
        let origins = parse_origins(DEFAULT_UNAUTHENTICATED_ORIGINS.iter().copied());
        assert_eq!(origins.len(), DEFAULT_UNAUTHENTICATED_ORIGINS.len());
        assert!(origins.contains(&origin("http://localhost:15275")));
        assert!(origins.contains(&origin("tauri://localhost")));
    }

    #[test]
    fn entries_are_normalized_and_deduplicated() {
        let origins = parse_origins(
            [
                " HTTP://LocalHost:15275/ ",
                "http://localhost:15275",
                "",
                "tauri://localhost",
            ]
            .into_iter(),
        );
        assert_eq!(
            origins.as_ref(),
            [
                origin("http://localhost:15275"),
                origin("tauri://localhost")
            ]
        );
    }

    #[test]
    fn malformed_entries_are_rejected() {
        for entry in [
            "*",
            "localhost:15275",
            "http://localhost:15275/app",
            "http://local host",
            "://localhost",
        ] {
            assert!(
                normalize_origin(entry).is_err(),
                "{entry} should be rejected"
            );
        }
    }

    #[test]
    fn any_origin_answers_every_request_with_a_wildcard() {
        let policy = CorsPolicy::AnyOrigin;
        assert_eq!(
            policy.allow_origin_value(Some(&origin("https://evil.example"))),
            Some(HeaderValue::from_static("*"))
        );
        assert_eq!(
            policy.allow_origin_value(None),
            Some(HeaderValue::from_static("*"))
        );
    }

    #[test]
    fn exact_policy_echoes_only_configured_origins() {
        let policy = exact(&["http://localhost:15275", "tauri://localhost"]);

        assert_eq!(
            policy.allow_origin_value(Some(&origin("http://localhost:15275"))),
            Some(origin("http://localhost:15275"))
        );
        assert_eq!(
            policy.allow_origin_value(Some(&origin("https://evil.example"))),
            None
        );
        assert_eq!(
            policy.allow_origin_value(Some(&origin("http://localhost:15276"))),
            None
        );
        assert_eq!(policy.allow_origin_value(None), None);
    }

    #[test]
    fn auth_state_selects_the_policy() {
        let origins = parse_origins(["http://localhost:15275"].into_iter());

        assert!(matches!(policy_for(true, &origins), CorsPolicy::AnyOrigin));
        assert!(matches!(
            policy_for(false, &origins),
            CorsPolicy::Exact(allowed) if allowed.as_ref() == [origin("http://localhost:15275")]
        ));
    }

    #[test]
    fn apply_allow_origin_varies_and_omits_disallowed_origins() {
        let policy = exact(&["http://localhost:15275"]);

        let mut allowed = HeaderMap::new();
        policy.apply_allow_origin(Some(&origin("http://localhost:15275")), &mut allowed);
        assert_eq!(
            allowed.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&origin("http://localhost:15275"))
        );
        assert_eq!(allowed.get(header::VARY), Some(&origin("origin")));

        let mut denied = HeaderMap::new();
        policy.apply_allow_origin(Some(&origin("https://evil.example")), &mut denied);
        assert!(denied.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        assert_eq!(denied.get(header::VARY), Some(&origin("origin")));
    }

    mod router_preflight {
        //! Drives `CorsPolicy::layer` through a router shaped like the one
        //! `ApiServer::build_router` assembles, so the browser-visible
        //! outcome of a preflight is asserted rather than the policy alone.

        use super::*;
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::post;
        use tower::ServiceExt;

        /// Same layer configuration as `ApiServer::build_router`, in front of
        /// a route that mutates state.
        fn app(policy: &CorsPolicy) -> Router {
            Router::new()
                .route("/api/pipeline/create", post(|| async { StatusCode::OK }))
                .layer(policy.layer().allow_methods(Any).allow_headers(Any))
        }

        async fn preflight(policy: &CorsPolicy, from: &str) -> axum::response::Response {
            app(policy)
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/api/pipeline/create")
                        .header(header::ORIGIN, from)
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                        .body(Body::empty())
                        .expect("test request should build"),
                )
                .await
                .expect("router call should be infallible")
        }

        #[tokio::test]
        async fn without_auth_only_configured_origins_are_allowed() {
            let policy = policy_for(
                false,
                &parse_origins(["http://localhost:15275"].into_iter()),
            );

            let allowed = preflight(&policy, "http://localhost:15275").await;
            assert_eq!(
                allowed.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                Some(&origin("http://localhost:15275"))
            );

            let denied = preflight(&policy, "https://evil.example").await;
            assert!(
                denied
                    .headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none(),
                "an unlisted origin must not be told the request is allowed"
            );
        }

        #[tokio::test]
        async fn with_auth_any_origin_still_preflights_successfully() {
            let policy = policy_for(true, &parse_origins(["http://localhost:15275"].into_iter()));

            for from in ["http://localhost:15275", "https://evil.example"] {
                let response = preflight(&policy, from).await;
                assert_eq!(
                    response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                    Some(&HeaderValue::from_static("*")),
                    "origin: {from}"
                );
            }
        }
    }
}
