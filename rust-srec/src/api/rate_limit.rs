//! Failed-login throttling for [`AuthService::authenticate`].
//!
//! [`AuthService::authenticate`]: crate::api::auth_service::AuthService::authenticate

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use tracing::warn;

/// Failures tolerated for one account inside the window before
/// `LoginRateLimiter::try_begin` starts rejecting. Overridden by
/// `API_LOGIN_MAX_FAILURES`.
pub const DEFAULT_MAX_FAILED_LOGIN_ATTEMPTS: usize = 5;

/// Sliding window over which failures are counted, and the time a key stays
/// blocked once its budget is spent. Overridden by `API_LOGIN_WINDOW_SECS`.
pub const DEFAULT_LOGIN_FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Failures tolerated from one source address inside the window. Much larger
/// than the per-account budget because a single address routinely carries
/// every login: the bundled frontend calls the API from its own server
/// process, and a reverse proxy replaces the browser's address with its own.
/// This budget caps Argon2id work; it is not a lockout. Overridden by
/// `API_LOGIN_IP_MAX_FAILURES`.
pub const DEFAULT_IP_MAX_FAILED_LOGIN_ATTEMPTS: usize = 100;

/// Env var overriding [`DEFAULT_MAX_FAILED_LOGIN_ATTEMPTS`].
pub const MAX_FAILURES_ENV: &str = "API_LOGIN_MAX_FAILURES";
/// Env var overriding [`DEFAULT_LOGIN_FAILURE_WINDOW`], in seconds.
pub const WINDOW_SECS_ENV: &str = "API_LOGIN_WINDOW_SECS";
/// Env var overriding [`DEFAULT_IP_MAX_FAILED_LOGIN_ATTEMPTS`].
pub const IP_MAX_FAILURES_ENV: &str = "API_LOGIN_IP_MAX_FAILURES";

/// Hard ceiling on tracked keys. `enforce_capacity` sweeps and then evicts
/// down to this many, so a caller cycling through usernames cannot grow the
/// map without bound.
const MAX_TRACKED_KEYS: usize = 10_000;

/// Minimum spacing between `sweep_expired` runs. A sweep takes every
/// `DashMap` shard's write lock, so it must not run once per request while
/// the map sits above capacity.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// What a failure counter is attributed to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LoginRateKey {
    /// SHA-256 of the normalised username. Digesting keeps the key a fixed 32
    /// bytes whatever the request body carried, so the map's cost per tracked
    /// account is not caller-controlled.
    Username([u8; 32]),
    /// Peer address from `ConnectInfo<SocketAddr>`.
    Ip(IpAddr),
}

impl LoginRateKey {
    /// Build the username key. Trimming and lowercasing first means `Alice`
    /// and `alice` share one counter.
    pub fn username(username: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(username.trim().to_lowercase().as_bytes());
        Self::Username(hasher.finalize().into())
    }
}

/// Failure timestamps for one key, oldest first.
///
/// `LoginRateLimiter::reserve_one` only pushes while the length is under the
/// key's budget, so the deque never exceeds it.
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
}

/// Sliding-window budget of failed logins, keyed by [`LoginRateKey`].
///
/// Every operation is synchronous and holds each `DashMap` shard guard only
/// for the statement that creates it, so no lock crosses an `.await`.
#[derive(Debug)]
pub struct LoginRateLimiter {
    entries: DashMap<LoginRateKey, FailureWindow>,
    username_max_failures: usize,
    ip_max_failures: usize,
    window: Duration,
    max_tracked_keys: usize,
    /// Reference point for `last_sweep`, since `Instant` is not atomic.
    epoch: Instant,
    /// Nanoseconds since `epoch` at the last sweep, biased by one so the
    /// initial `0` reads as "never swept" and the first over-capacity insert
    /// sweeps immediately.
    last_sweep: AtomicU64,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_FAILED_LOGIN_ATTEMPTS,
            DEFAULT_IP_MAX_FAILED_LOGIN_ATTEMPTS,
            DEFAULT_LOGIN_FAILURE_WINDOW,
            MAX_TRACKED_KEYS,
        )
    }
}

impl LoginRateLimiter {
    /// Create a limiter with explicit settings. Production callers use
    /// [`LoginRateLimiter::from_env`].
    pub fn new(
        username_max_failures: usize,
        ip_max_failures: usize,
        window: Duration,
        max_tracked_keys: usize,
    ) -> Self {
        Self {
            entries: DashMap::new(),
            username_max_failures: username_max_failures.max(1),
            ip_max_failures: ip_max_failures.max(1),
            window,
            max_tracked_keys: max_tracked_keys.max(1),
            epoch: Instant::now(),
            last_sweep: AtomicU64::new(0),
        }
    }

    /// Read the budgets from `API_LOGIN_MAX_FAILURES`,
    /// `API_LOGIN_IP_MAX_FAILURES`, and `API_LOGIN_WINDOW_SECS`, warning and
    /// keeping the default for a value that is not a positive integer.
    pub fn from_env() -> Self {
        let window_secs = positive_env(
            WINDOW_SECS_ENV,
            DEFAULT_LOGIN_FAILURE_WINDOW.as_secs() as usize,
        ) as u64;
        Self::new(
            positive_env(MAX_FAILURES_ENV, DEFAULT_MAX_FAILED_LOGIN_ATTEMPTS),
            positive_env(IP_MAX_FAILURES_ENV, DEFAULT_IP_MAX_FAILED_LOGIN_ATTEMPTS),
            Duration::from_secs(window_secs),
            MAX_TRACKED_KEYS,
        )
    }

    fn max_failures(&self, key: &LoginRateKey) -> usize {
        match key {
            LoginRateKey::Username(_) => self.username_max_failures,
            LoginRateKey::Ip(_) => self.ip_max_failures,
        }
    }

    /// Take one failure slot on every key, or report how long the caller must
    /// wait. The returned `Instant` identifies the reserved slot and must be
    /// handed back to [`Self::commit_success`] or [`Self::release`].
    ///
    /// Reserving up front is what makes the budget hold under concurrency:
    /// the count is incremented under the same shard guard that reads it, so
    /// a burst of simultaneous requests cannot all observe a sub-limit count
    /// and go on to `verify_password_blocking`.
    pub fn try_begin(&self, keys: &[LoginRateKey]) -> Result<Instant, Duration> {
        self.try_begin_at(keys, Instant::now())
    }

    fn try_begin_at(&self, keys: &[LoginRateKey], now: Instant) -> Result<Instant, Duration> {
        for (index, key) in keys.iter().enumerate() {
            if let Err(retry_after) = self.reserve_one(key, now) {
                // Keys are reserved one at a time; give back the ones already
                // taken so a rejection on a later key does not consume them.
                self.release(&keys[..index], now);
                return Err(retry_after);
            }
        }
        self.enforce_capacity(now);
        Ok(now)
    }

    /// Prune and either take a slot or report the wait, under a single shard
    /// guard held across the whole read-modify-write.
    fn reserve_one(&self, key: &LoginRateKey, now: Instant) -> Result<(), Duration> {
        let max = self.max_failures(key);
        let mut entry = self.entries.entry(key.clone()).or_default();
        entry.prune(now, self.window);
        if entry.failures.len() >= max {
            // Everything left is inside the window, so the front entry is the
            // first one that will expire.
            let oldest = entry.failures.front().copied().unwrap_or(now);
            return Err(self.window.saturating_sub(now.duration_since(oldest)));
        }
        entry.failures.push_back(now);
        Ok(())
    }

    /// Give back a slot reserved by [`Self::try_begin`] on every key, for
    /// outcomes the caller could not have avoided (a repository or hashing
    /// failure rather than a wrong credential).
    pub fn release(&self, keys: &[LoginRateKey], reserved_at: Instant) {
        for key in keys {
            let mut emptied = false;
            if let Some(mut entry) = self.entries.get_mut(key)
                && let Some(index) = entry
                    .failures
                    .iter()
                    .rposition(|failure| *failure == reserved_at)
            {
                entry.failures.remove(index);
                emptied = entry.failures.is_empty();
            }
            // The `get_mut` guard ended with the block above; `remove` takes
            // its own shard lock.
            if emptied {
                self.entries.remove(key);
            }
        }
    }

    /// Settle a successful login.
    ///
    /// The account's whole budget is cleared, so a user who mistyped a few
    /// times starts over. The address only gets its own attempt back: one
    /// account the caller does hold must not reset the budget they are
    /// spending on the others.
    pub fn commit_success(&self, keys: &[LoginRateKey], reserved_at: Instant) {
        for key in keys {
            match key {
                LoginRateKey::Username(_) => {
                    self.entries.remove(key);
                }
                LoginRateKey::Ip(_) => self.release(std::slice::from_ref(key), reserved_at),
            }
        }
    }

    /// Bring the map back under `max_tracked_keys`, at most once per
    /// [`SWEEP_INTERVAL`].
    fn enforce_capacity(&self, now: Instant) {
        if self.entries.len() <= self.max_tracked_keys {
            return;
        }

        let now_nanos = now
            .duration_since(self.epoch)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        let last = self.last_sweep.load(Ordering::Acquire);
        if last != 0 && now_nanos.saturating_sub(last - 1) < SWEEP_INTERVAL.as_nanos() as u64 {
            return;
        }
        if self
            .last_sweep
            .compare_exchange(
                last,
                now_nanos.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            // Another caller claimed this sweep.
            return;
        }

        self.sweep_expired(now);
        self.evict_oldest_beyond_capacity();
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

    /// Evict the least recently active keys until the map fits, so a sweep
    /// that frees nothing still leaves a bounded map.
    fn evict_oldest_beyond_capacity(&self) {
        let excess = self.entries.len().saturating_sub(self.max_tracked_keys);
        if excess == 0 {
            return;
        }
        // Collected first: the iterator holds the shard guards `remove` needs.
        let mut activity: Vec<(LoginRateKey, Option<Instant>)> = self
            .entries
            .iter()
            .map(|entry| (entry.key().clone(), entry.failures.back().copied()))
            .collect();
        activity.sort_unstable_by_key(|(_, newest)| *newest);
        for (key, _) in activity.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }

    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.entries.len()
    }
}

/// Parse a positive integer env var, warning and falling back otherwise.
fn positive_env(name: &str, default: usize) -> usize {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return default;
    }
    match raw.parse::<usize>() {
        Ok(value) if value > 0 => value,
        _ => {
            warn!(
                env = name,
                value = %raw,
                default,
                "Expected a positive integer; keeping the default"
            );
            default
        }
    }
}

/// Peer address of the connection a request arrived on, or `None` when the
/// router was not served with
/// `Router::into_make_service_with_connect_info::<SocketAddr>` (unit tests
/// calling handlers through `tower::ServiceExt::oneshot`).
///
/// Deliberately ignores `X-Forwarded-For` / `X-Real-IP`: the server has no
/// trusted-proxy configuration, so an attacker could otherwise mint a fresh
/// budget per request by varying the header. The flip side is that behind any
/// reverse proxy — including the frontend container this project ships —
/// every login shares one address, which is why
/// [`DEFAULT_IP_MAX_FAILED_LOGIN_ATTEMPTS`] is a CPU cap and not a lockout.
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

    /// Account budget 3, address budget 6, so the two are distinguishable.
    fn limiter() -> LoginRateLimiter {
        LoginRateLimiter::new(3, 6, Duration::from_secs(60), 8)
    }

    fn alice() -> Vec<LoginRateKey> {
        vec![LoginRateKey::username("Alice")]
    }

    fn ip(address: &str) -> LoginRateKey {
        LoginRateKey::Ip(address.parse().expect("test address should be a valid IP"))
    }

    #[test]
    fn username_key_is_case_insensitive_and_fixed_size() {
        assert_eq!(
            LoginRateKey::username("  AlIcE "),
            LoginRateKey::username("alice")
        );
        assert_ne!(
            LoginRateKey::username("alice"),
            LoginRateKey::username("alicia")
        );
        // A 4 KiB username costs the same key as a short one.
        assert_eq!(
            std::mem::size_of_val(&LoginRateKey::username(&"x".repeat(4096))),
            std::mem::size_of::<LoginRateKey>()
        );
    }

    #[test]
    fn blocks_only_after_the_budget_is_spent() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            limiter
                .try_begin_at(&alice(), now)
                .expect("attempts within the budget are admitted");
        }

        let retry_after = limiter
            .try_begin_at(&alice(), now)
            .expect_err("the fourth attempt should be blocked");
        assert!(retry_after <= Duration::from_secs(60));
        assert!(retry_after > Duration::from_secs(59));
    }

    #[test]
    fn block_lifts_once_the_window_elapses() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            limiter.try_begin_at(&alice(), now).expect("within budget");
        }
        assert!(limiter.try_begin_at(&alice(), now).is_err());
        assert!(
            limiter
                .try_begin_at(&alice(), now + Duration::from_secs(59))
                .is_err(),
            "failures inside the window still count"
        );
        assert!(
            limiter
                .try_begin_at(&alice(), now + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn keys_are_isolated_from_each_other() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            limiter.try_begin_at(&alice(), now).expect("within budget");
        }

        assert!(limiter.try_begin_at(&alice(), now).is_err());
        assert!(
            limiter
                .try_begin_at(&[LoginRateKey::username("bob")], now)
                .is_ok()
        );
        assert!(limiter.try_begin_at(&[ip("10.0.0.1")], now).is_ok());
    }

    #[test]
    fn address_budget_is_far_looser_than_the_account_budget() {
        let limiter = limiter();
        let now = Instant::now();
        let address = ip("192.0.2.9");

        // Six distinct accounts from one address: each keeps its own budget,
        // and the address budget is what eventually stops them.
        for index in 0..6 {
            limiter
                .try_begin_at(
                    &[
                        LoginRateKey::username(&format!("user{index}")),
                        address.clone(),
                    ],
                    now,
                )
                .expect("the address budget is 6");
        }

        assert!(
            limiter
                .try_begin_at(&[LoginRateKey::username("user6"), address.clone()], now)
                .is_err(),
            "a seventh attempt exhausts the address budget"
        );
        assert!(
            limiter
                .try_begin_at(&[LoginRateKey::username("user6")], now)
                .is_ok(),
            "rejecting on the address must not have consumed the username slot"
        );
    }

    #[test]
    fn success_clears_the_account_but_only_refunds_the_address() {
        let limiter = limiter();
        let now = Instant::now();
        let address = ip("198.51.100.4");
        let keys = vec![LoginRateKey::username("alice"), address.clone()];

        for _ in 0..2 {
            limiter.try_begin_at(&keys, now).expect("within budget");
        }
        let reserved = limiter.try_begin_at(&keys, now).expect("within budget");
        limiter.commit_success(&keys, reserved);

        assert!(
            limiter.try_begin_at(&alice(), now).is_ok(),
            "the account budget was cleared"
        );
        // Two earlier failures remain on the address, so four of its six slots
        // are still free.
        for _ in 0..4 {
            limiter
                .try_begin_at(std::slice::from_ref(&address), now)
                .expect("the address keeps only its earlier failures");
        }
        assert!(
            limiter
                .try_begin_at(std::slice::from_ref(&address), now)
                .is_err()
        );
    }

    #[test]
    fn release_hands_the_slot_back() {
        let limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            let reserved = limiter.try_begin_at(&alice(), now).expect("within budget");
            limiter.release(&alice(), reserved);
        }
        assert!(
            limiter.try_begin_at(&alice(), now).is_ok(),
            "released attempts do not count against the budget"
        );
    }

    #[test]
    fn concurrent_attempts_cannot_exceed_the_budget() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let limiter = Arc::new(LoginRateLimiter::new(
            5,
            1_000,
            Duration::from_secs(600),
            64,
        ));
        let admitted = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for _ in 0..64 {
                let limiter = Arc::clone(&limiter);
                let admitted = Arc::clone(&admitted);
                scope.spawn(move || {
                    if limiter
                        .try_begin(&[LoginRateKey::username("admin")])
                        .is_ok()
                    {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });

        assert_eq!(
            admitted.load(Ordering::Relaxed),
            5,
            "exactly the budget may pass a concurrent check-and-reserve"
        );
    }

    #[test]
    fn capacity_is_a_hard_cap_even_when_nothing_is_stale() {
        let limiter = LoginRateLimiter::new(3, 1_000, Duration::from_secs(600), 4);
        let now = Instant::now();

        for index in 0..12 {
            // Stepping past SWEEP_INTERVAL each time keeps the throttle from
            // skipping, and gives eviction a defined oldest entry.
            let at = now + Duration::from_secs(index * 31);
            limiter
                .try_begin_at(&[LoginRateKey::username(&format!("user{index}"))], at)
                .expect("each username has its own budget");
        }

        assert!(
            limiter.tracked_keys() <= 5,
            "the map stays at capacity plus the key just inserted: {}",
            limiter.tracked_keys()
        );
        assert!(
            limiter
                .try_begin_at(&[LoginRateKey::username("user0")], now)
                .is_ok(),
            "the least recently active keys were the ones evicted"
        );
    }

    #[test]
    fn sweeping_is_throttled() {
        let limiter = LoginRateLimiter::new(3, 1_000, Duration::from_secs(600), 2);
        let now = Instant::now();

        for index in 0..8u64 {
            limiter
                .try_begin_at(
                    &[LoginRateKey::username(&format!("user{index}"))],
                    now + Duration::from_millis(index),
                )
                .expect("each username has its own budget");
        }

        // The first over-capacity insert sweeps and evicts; the rest land
        // inside SWEEP_INTERVAL and are left to accumulate.
        assert!(limiter.tracked_keys() > 2);
        limiter
            .try_begin_at(
                &[LoginRateKey::username("later")],
                now + SWEEP_INTERVAL + Duration::from_secs(1),
            )
            .expect("within budget");
        assert!(
            limiter.tracked_keys() <= 3,
            "the next sweep after the interval brings the map back to capacity: {}",
            limiter.tracked_keys()
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
}
