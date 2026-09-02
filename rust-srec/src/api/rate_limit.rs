//! Failed-login throttling for [`AuthService::authenticate`].
//!
//! [`AuthService::authenticate`]: crate::api::auth_service::AuthService::authenticate

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use dashmap::DashMap;

/// Failures tolerated per key inside [`LOGIN_FAILURE_WINDOW`] before
/// `LoginRateLimiter::check` starts rejecting.
pub const MAX_FAILED_LOGIN_ATTEMPTS: usize = 5;

/// Sliding window over which failures are counted, and the time a key stays
/// blocked after the limit is reached.
pub const LOGIN_FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Number of tracked keys above which `LoginRateLimiter::record_failure`
/// sweeps entries whose window has fully elapsed. Bounds the map for a caller
/// that cycles through usernames or source addresses.
const MAX_TRACKED_KEYS: usize = 10_000;

/// What a failure counter is attributed to.
///
/// `authenticate` counts every attempt against both the username and, when
/// the peer address is known, the caller's IP, so neither a single account
/// nor a single source can absorb unlimited Argon2id verifications.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LoginRateKey {
    /// Lowercased username as submitted; tracked even when no such user row
    /// exists so a miss cannot be distinguished from a wrong password.
    Username(String),
    /// Peer address from `ConnectInfo<SocketAddr>`.
    Ip(IpAddr),
}

impl LoginRateKey {
    /// Build the username key, normalising case so `Alice` and `alice` share
    /// one counter.
    pub fn username(username: &str) -> Self {
        Self::Username(username.trim().to_lowercase())
    }
}

/// Failure timestamps for one key, newest last, holding at most
/// `max_failures` entries.
#[derive(Debug, Default)]
struct FailureWindow {
    failures: VecDeque<Instant>,
}

impl FailureWindow {
    /// Drop failures older than `window`, keeping the deque ordered.
    fn prune(&mut self, now: Instant, window: Duration) {
        while let Some(oldest) = self.failures.front() {
            if now.duration_since(*oldest) >= window {
                self.failures.pop_front();
            } else {
                break;
            }
        }
    }

    /// Time until the oldest failure still inside `window` leaves it, or
    /// `None` while fewer than `max_failures` failures remain in the window.
    ///
    /// Read-only, so it cannot prune: expired entries are skipped instead and
    /// removed by the next `prune` or `sweep_expired`.
    fn retry_after(&self, now: Instant, window: Duration, max_failures: usize) -> Option<Duration> {
        let oldest_live = self
            .failures
            .iter()
            .position(|at| now.duration_since(*at) < window)?;
        if self.failures.len() - oldest_live < max_failures {
            return None;
        }
        let oldest = self.failures[oldest_live];
        Some(window.saturating_sub(now.duration_since(oldest)))
    }
}

/// Sliding-window counter of failed logins, keyed by [`LoginRateKey`].
///
/// All operations are synchronous and take no lock across an `.await`; the
/// `DashMap` shard guards are temporaries of the statements that create them.
#[derive(Debug)]
pub struct LoginRateLimiter {
    entries: DashMap<LoginRateKey, FailureWindow>,
    max_failures: usize,
    window: Duration,
    max_tracked_keys: usize,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new(
            MAX_FAILED_LOGIN_ATTEMPTS,
            LOGIN_FAILURE_WINDOW,
            MAX_TRACKED_KEYS,
        )
    }
}

impl LoginRateLimiter {
    /// Create a limiter with explicit settings. Production callers use
    /// [`LoginRateLimiter::default`].
    pub fn new(max_failures: usize, window: Duration, max_tracked_keys: usize) -> Self {
        Self {
            entries: DashMap::new(),
            max_failures: max_failures.max(1),
            window,
            max_tracked_keys: max_tracked_keys.max(1),
        }
    }

    /// Return the wait imposed by whichever of `keys` is furthest from
    /// leaving its window, or `None` when every key is still under the limit.
    pub fn check(&self, keys: &[LoginRateKey]) -> Option<Duration> {
        self.check_at(keys, Instant::now())
    }

    fn check_at(&self, keys: &[LoginRateKey], now: Instant) -> Option<Duration> {
        keys.iter()
            .filter_map(|key| {
                // The `DashMap::get` shard guard is a temporary of this
                // statement chain, so no lock outlives `check_at`.
                self.entries
                    .get(key)?
                    .retry_after(now, self.window, self.max_failures)
            })
            .max()
    }

    /// Count one failed attempt against every key.
    pub fn record_failure(&self, keys: &[LoginRateKey]) {
        self.record_failure_at(keys, Instant::now());
    }

    fn record_failure_at(&self, keys: &[LoginRateKey], now: Instant) {
        for key in keys {
            let mut entry = self.entries.entry(key.clone()).or_default();
            entry.prune(now, self.window);
            entry.failures.push_back(now);
            while entry.failures.len() > self.max_failures {
                entry.failures.pop_front();
            }
        }
        // The `entry` guards above are dropped before the sweep, which takes
        // its own shard locks via `DashMap::retain`.
        if self.entries.len() > self.max_tracked_keys {
            self.sweep_expired(now);
        }
    }

    /// Forget the counters for `key`; called after a successful login so a
    /// user who mistyped a few times starts over.
    pub fn clear(&self, key: &LoginRateKey) {
        self.entries.remove(key);
    }

    /// Drop keys whose most recent failure has left the window.
    fn sweep_expired(&self, now: Instant) {
        let window = self.window;
        self.entries.retain(|_, entry| {
            entry
                .failures
                .back()
                .is_some_and(|newest| now.duration_since(*newest) < window)
        });
    }

    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.entries.len()
    }
}

/// Peer address of the connection a request arrived on, or `None` when the
/// router was not served with
/// `Router::into_make_service_with_connect_info::<SocketAddr>` (unit tests
/// calling handlers through `tower::ServiceExt::oneshot`).
///
/// Deliberately ignores `X-Forwarded-For` / `X-Real-IP`: the server has no
/// trusted-proxy configuration, so an attacker could otherwise mint a fresh
/// rate-limit bucket per request by varying the header.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(addr)| addr.ip()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> LoginRateLimiter {
        LoginRateLimiter::new(3, Duration::from_secs(60), 8)
    }

    fn alice() -> Vec<LoginRateKey> {
        vec![LoginRateKey::username("Alice")]
    }

    #[test]
    fn username_key_is_case_insensitive() {
        assert_eq!(
            LoginRateKey::username("  AlIcE "),
            LoginRateKey::Username("alice".to_string())
        );
    }

    #[test]
    fn blocks_only_after_the_limit_is_reached() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..2 {
            limiter.record_failure_at(&alice(), now);
            assert!(limiter.check_at(&alice(), now).is_none());
        }

        limiter.record_failure_at(&alice(), now);
        let retry_after = limiter
            .check_at(&alice(), now)
            .expect("the third failure should block");
        assert!(retry_after <= Duration::from_secs(60));
        assert!(retry_after > Duration::from_secs(59));
    }

    #[test]
    fn block_lifts_once_the_window_elapses() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            limiter.record_failure_at(&alice(), now);
        }
        assert!(limiter.check_at(&alice(), now).is_some());

        let later = now + Duration::from_secs(59);
        assert!(
            limiter.check_at(&alice(), later).is_some(),
            "failures inside the window still count"
        );

        let after_window = now + Duration::from_secs(60);
        assert!(limiter.check_at(&alice(), after_window).is_none());
    }

    #[test]
    fn keys_are_isolated_from_each_other() {
        let limiter = limiter();
        let now = Instant::now();
        let bob = vec![LoginRateKey::username("bob")];
        let ip = vec![LoginRateKey::Ip("10.0.0.1".parse().expect("valid IP"))];

        for _ in 0..3 {
            limiter.record_failure_at(&alice(), now);
        }

        assert!(limiter.check_at(&alice(), now).is_some());
        assert!(limiter.check_at(&bob, now).is_none());
        assert!(limiter.check_at(&ip, now).is_none());
    }

    #[test]
    fn ip_key_blocks_across_different_usernames() {
        let limiter = limiter();
        let now = Instant::now();
        let ip = LoginRateKey::Ip("192.0.2.9".parse().expect("valid IP"));

        for name in ["a", "b", "c"] {
            limiter.record_failure_at(&[LoginRateKey::username(name), ip.clone()], now);
        }

        assert!(
            limiter
                .check_at(&[LoginRateKey::username("d"), ip.clone()], now)
                .is_some(),
            "a fourth username from the same address is blocked by the IP key"
        );
        assert!(
            limiter
                .check_at(&[LoginRateKey::username("d")], now)
                .is_none(),
            "the username itself never failed"
        );
    }

    #[test]
    fn success_clears_the_key() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            limiter.record_failure_at(&alice(), now);
        }
        assert!(limiter.check_at(&alice(), now).is_some());

        limiter.clear(&LoginRateKey::username("alice"));
        assert!(limiter.check_at(&alice(), now).is_none());
    }

    #[test]
    fn check_reports_the_longest_remaining_wait() {
        let limiter = limiter();
        let now = Instant::now();
        let ip = LoginRateKey::Ip("198.51.100.4".parse().expect("valid IP"));

        for _ in 0..3 {
            limiter.record_failure_at(&alice(), now);
        }
        let later = now + Duration::from_secs(30);
        for _ in 0..3 {
            limiter.record_failure_at(std::slice::from_ref(&ip), later);
        }

        let retry_after = limiter
            .check_at(&[LoginRateKey::username("alice"), ip], later)
            .expect("both keys are blocked");
        assert!(
            retry_after > Duration::from_secs(59),
            "the IP key, blocked 30s later, dictates the wait: {retry_after:?}"
        );
    }

    mod client_ip {
        //! Pins what `ClientIp` reads: the peer address of the accepted
        //! connection, and nothing derived from request headers.

        use super::*;
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::get;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        fn app() -> Router {
            Router::new().route(
                "/ip",
                get(|ClientIp(ip): ClientIp| async move {
                    match ip {
                        Some(ip) => ip.to_string(),
                        None => "none".to_string(),
                    }
                }),
            )
        }

        #[tokio::test]
        async fn resolves_to_the_peer_address_of_a_served_connection() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("loopback bind should succeed");
            let addr = listener.local_addr().expect("listener should have an addr");
            tokio::spawn(async move {
                let _ = axum::serve(
                    listener,
                    app().into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await;
            });

            // A raw request keeps the test off any HTTP client's TLS setup.
            let mut stream = tokio::net::TcpStream::connect(addr)
                .await
                .expect("test server should accept the connection");
            stream
                .write_all(
                    format!("GET /ip HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .expect("request should be written");
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .await
                .expect("response should be readable");
            assert!(
                response.ends_with("127.0.0.1"),
                "handler should have seen the loopback peer address: {response}"
            );
        }

        #[tokio::test]
        async fn forwarding_headers_are_not_trusted() {
            // Served without connect info, so any address in the response
            // could only have come from a header.
            let response = app()
                .oneshot(
                    Request::builder()
                        .uri("/ip")
                        .header("x-forwarded-for", "203.0.113.7")
                        .header("x-real-ip", "203.0.113.8")
                        .body(Body::empty())
                        .expect("test request should build"),
                )
                .await
                .expect("router call should be infallible");
            assert_eq!(response.status(), StatusCode::OK);

            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body should be readable");
            assert_eq!(&body[..], b"none");
        }
    }

    #[test]
    fn stale_keys_are_swept_once_the_map_grows() {
        let limiter = LoginRateLimiter::new(3, Duration::from_secs(60), 4);
        let now = Instant::now();

        for index in 0..5 {
            limiter.record_failure_at(&[LoginRateKey::username(&format!("user{index}"))], now);
        }
        assert_eq!(limiter.tracked_keys(), 5, "nothing is stale yet");

        let after_window = now + Duration::from_secs(120);
        for index in 5..10 {
            limiter.record_failure_at(
                &[LoginRateKey::username(&format!("user{index}"))],
                after_window,
            );
        }
        assert!(
            limiter.tracked_keys() < 10,
            "the sweep dropped keys whose window elapsed"
        );
        assert!(
            limiter
                .check_at(&[LoginRateKey::username("user0")], after_window)
                .is_none()
        );
    }
}
