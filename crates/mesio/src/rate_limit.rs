//! Back-pressure for servers that rate limit, shared by every request made
//! through one [`ClientPool`](crate::downloader::ClientPool).
//!
//! A `429 Too Many Requests`, or a `503 Service Unavailable` that carries
//! `Retry-After`, puts the answering origin into a cool-down. Manifest,
//! playlist, Segment Index, key and segment requests to that origin wait the
//! cool-down out before they are sent. When it ends, one request goes first
//! as a probe and the others wait for its answer, so a server that is
//! recovering does not receive the whole backlog at once. Any answer to a
//! request sent after the cool-down that is not itself rate limiting ends it.
//!
//! The cool-down lasts as long as `Retry-After` asks (delay-seconds or an
//! HTTP date), up to [`RateLimitPolicy::max_retry_after`]. Without it, it
//! backs off exponentially from [`RateLimitPolicy::base_backoff`], doubling
//! for each probe that is rate limited again up to
//! [`RateLimitPolicy::max_backoff`], with equal jitter.
//!
//! Rate limiting is not counted against the retry budgets of segments and
//! manifest refreshes, which are meant for failures that waiting does not
//! cure. It is bounded by [`RateLimitPolicy::max_throttled`] instead: a
//! segment fails once it has waited that long in total, a manifest or
//! playlist once that long has passed since its last successful refresh.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rand::RngExt;
use reqwest::StatusCode;
use reqwest::header::{DATE, HeaderMap, RETRY_AFTER};
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use url::Url;

use crate::DownloadError;
use crate::dash::manifest::parse_http_date;

/// How long a waiting request sleeps before it re-checks a probe that has
/// not answered yet. Every state change also wakes it; this only bounds the
/// wait should the probe's answer be lost to a bug.
const PROBE_RECHECK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateLimitPolicy {
    /// First cool-down without `Retry-After`.
    pub base_backoff: Duration,
    /// Longest cool-down without `Retry-After`.
    pub max_backoff: Duration,
    /// Longest cool-down `Retry-After` can ask for. Waiting longer would
    /// leave a gap of at least that length in a live recording, while
    /// probing early costs one request that is rate limited again.
    pub max_retry_after: Duration,
    /// How long a stream rides out rate limiting before it fails (see the
    /// module documentation).
    pub max_throttled: Duration,
}

impl Default for RateLimitPolicy {
    fn default() -> Self {
        Self {
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(60),
            max_throttled: Duration::from_secs(5 * 60),
        }
    }
}

impl RateLimitPolicy {
    /// The cool-down after `strikes` consecutive rate-limited probes without
    /// `Retry-After`, with `jitter` in `[0, 1]` choosing a point in its upper
    /// half. Equal jitter keeps sessions that were limited together from
    /// probing together, without ever retrying sooner than half the backoff.
    fn backoff(&self, strikes: u32, jitter: f64) -> Duration {
        let exponent = strikes.saturating_sub(1).min(16);
        let full = self
            .base_backoff
            .checked_mul(1 << exponent)
            .unwrap_or(self.max_backoff)
            .min(self.max_backoff);
        let half = full / 2;
        half + half.mul_f64(jitter.clamp(0.0, 1.0))
    }
}

/// Whether `status` asks the client to slow down. A `503` without
/// `Retry-After` is an ordinary server error.
pub(crate) fn is_rate_limited(status: StatusCode, has_retry_after: bool) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::SERVICE_UNAVAILABLE && has_retry_after)
}

/// The wait a `Retry-After` header asks for (RFC 9110 §10.2.3), uncapped.
/// An HTTP date is measured against the response's own `Date` when it has
/// one, so a skewed local clock does not shift it; `now` is the fallback. A
/// date in the past means no wait. `None` when absent or unparseable.
pub(crate) fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        // Only an overflow fails to parse: all digits.
        return Some(
            value
                .parse::<u64>()
                .map_or(Duration::MAX, Duration::from_secs),
        );
    }
    let at = parse_http_date(value).ok()?;
    let reference = headers
        .get(DATE)
        .and_then(|date| date.to_str().ok())
        .and_then(|date| parse_http_date(date).ok())
        .unwrap_or(now);
    Some((at - reference).to_std().unwrap_or(Duration::ZERO))
}

#[derive(Debug)]
struct Cooldown {
    /// Consecutive rate-limited answers to requests sent outside a
    /// cool-down; what the backoff grows with.
    strikes: u32,
    until: Instant,
    /// The probe sent after `until`, while its answer is pending.
    probe: Option<u64>,
}

#[derive(Debug, Default)]
struct State {
    /// By origin (scheme, host and port).
    cooldowns: HashMap<String, Cooldown>,
    next_probe: u64,
}

#[derive(Debug)]
pub(crate) struct RateLimiter {
    policy: RateLimitPolicy,
    state: Mutex<State>,
    changed: Notify,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimitPolicy::default())
    }
}

/// Held by a request from before it is sent until its answer has been
/// passed to [`RateLimiter::observe`]. A probe dropped without an answer
/// (a network error, a timeout, cancellation) hands probing to the next
/// waiting request.
#[must_use = "a probe is released as soon as its admission is dropped"]
pub(crate) struct Admission<'a> {
    probe: Option<(&'a RateLimiter, String, u64)>,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if let Some((limiter, origin, id)) = self.probe.take() {
            limiter.release_probe(&origin, id);
        }
    }
}

impl RateLimiter {
    pub(crate) fn new(policy: RateLimitPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        }
    }

    pub(crate) fn policy(&self) -> &RateLimitPolicy {
        &self.policy
    }

    /// Waits until a request to `url` may be sent: at once unless its origin
    /// is cooling down. Fails only when `cancel` fires.
    pub(crate) async fn admit(
        &self,
        url: &Url,
        cancel: &CancellationToken,
    ) -> Result<Admission<'_>, DownloadError> {
        let origin = origin_key(url);
        loop {
            // Registered before the state is read, so a change between the
            // read and the wait still wakes this request.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let wake_at = {
                let mut state = self.state.lock();
                let next_probe = state.next_probe;
                let Some(cooldown) = state.cooldowns.get_mut(&origin) else {
                    return Ok(Admission { probe: None });
                };
                let now = Instant::now();
                if now < cooldown.until {
                    cooldown.until
                } else if cooldown.probe.is_none() {
                    cooldown.probe = Some(next_probe);
                    state.next_probe = next_probe.wrapping_add(1);
                    debug!(origin, "rate limit cool-down over; probing");
                    return Ok(Admission {
                        probe: Some((self, origin, next_probe)),
                    });
                } else {
                    now + PROBE_RECHECK
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return Err(DownloadError::Cancelled),
                () = &mut notified => {}
                () = tokio::time::sleep_until(wake_at) => {}
            }
        }
    }

    /// Records the answer to a request to `url`. Returns how long the origin
    /// now cools down when the answer is rate limiting, `None` otherwise.
    pub(crate) fn observe(
        &self,
        url: &Url,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Duration> {
        let retry_after = retry_after(headers, Utc::now());
        let origin = origin_key(url);
        let now = Instant::now();
        let mut state = self.state.lock();
        if !is_rate_limited(status, retry_after.is_some()) {
            // An answer to a request sent before the cool-down began says
            // nothing about the server since; it must not end the cool-down.
            if state
                .cooldowns
                .get(&origin)
                .is_some_and(|cooldown| now >= cooldown.until)
            {
                state.cooldowns.remove(&origin);
                drop(state);
                self.changed.notify_waiters();
                debug!(origin, "server stopped rate limiting");
            }
            return None;
        }

        let capped = retry_after.map(|wait| wait.min(self.policy.max_retry_after));
        let (delay, strikes) = match state.cooldowns.get_mut(&origin) {
            // Concurrent requests sent before the cool-down began: they add
            // no evidence that the server needs longer, unless it says so.
            Some(cooldown) if now < cooldown.until => {
                if let Some(wait) = capped {
                    cooldown.until = cooldown.until.max(now + wait);
                }
                return Some(cooldown.until.saturating_duration_since(now));
            }
            cooldown => {
                let strikes = cooldown.map_or(1, |cooldown| cooldown.strikes.saturating_add(1));
                let delay = capped.unwrap_or_else(|| {
                    self.policy
                        .backoff(strikes, rand::rng().random_range(0.0..=1.0))
                });
                (delay, strikes)
            }
        };
        state.cooldowns.insert(
            origin.clone(),
            Cooldown {
                strikes,
                until: now + delay,
                probe: None,
            },
        );
        drop(state);
        self.changed.notify_waiters();
        warn!(
            origin,
            status = status.as_u16(),
            strikes,
            retry_after_secs = retry_after.map(|wait| wait.as_secs()),
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "server is rate limiting; pausing requests to it"
        );
        Some(delay)
    }

    /// The rest of the cool-down of `url`'s origin when `error` is a
    /// rate-limiting answer from it. `Some(ZERO)` while a probe is due.
    pub(crate) fn throttled(&self, url: &Url, error: &DownloadError) -> Option<Duration> {
        let DownloadError::HttpStatus { status, .. } = error else {
            return None;
        };
        if !is_rate_limited(*status, true) {
            return None;
        }
        let state = self.state.lock();
        state
            .cooldowns
            .get(&origin_key(url))
            .map(|cooldown| cooldown.until.saturating_duration_since(Instant::now()))
    }

    fn release_probe(&self, origin: &str, id: u64) {
        let mut state = self.state.lock();
        if let Some(cooldown) = state.cooldowns.get_mut(origin)
            && cooldown.probe == Some(id)
        {
            cooldown.probe = None;
            drop(state);
            self.changed.notify_waiters();
        }
    }
}

fn origin_key(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::header::HeaderValue;

    use super::*;

    fn headers(pairs: &[(reqwest::header::HeaderName, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn at(value: &str) -> DateTime<Utc> {
        parse_http_date(value).unwrap()
    }

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn rate_limiting_statuses() {
        assert!(is_rate_limited(StatusCode::TOO_MANY_REQUESTS, false));
        assert!(is_rate_limited(StatusCode::TOO_MANY_REQUESTS, true));
        assert!(is_rate_limited(StatusCode::SERVICE_UNAVAILABLE, true));
        assert!(!is_rate_limited(StatusCode::SERVICE_UNAVAILABLE, false));
        for status in [
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            assert!(!is_rate_limited(status, true), "{status}");
        }
    }

    #[test]
    fn retry_after_reads_delay_seconds() {
        let now = at("Wed, 14 Oct 2026 12:00:00 GMT");
        assert_eq!(
            retry_after(&headers(&[(RETRY_AFTER, "120")]), now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            retry_after(&headers(&[(RETRY_AFTER, " 0 ")]), now),
            Some(Duration::ZERO)
        );
        assert_eq!(
            retry_after(&headers(&[(RETRY_AFTER, "99999999999999999999999")]), now),
            Some(Duration::MAX),
            "an overflowing delay saturates; the limiter caps it"
        );
    }

    #[test]
    fn retry_after_reads_http_dates_against_the_response_date() {
        let local = at("Wed, 14 Oct 2026 12:00:00 GMT");
        let wait = headers(&[(RETRY_AFTER, "Wed, 14 Oct 2026 12:00:30 GMT")]);
        assert_eq!(retry_after(&wait, local), Some(Duration::from_secs(30)));
        // The server's clock is 20 s ahead of ours: its date is the reference.
        let skewed = headers(&[
            (RETRY_AFTER, "Wed, 14 Oct 2026 12:00:30 GMT"),
            (DATE, "Wed, 14 Oct 2026 12:00:20 GMT"),
        ]);
        assert_eq!(retry_after(&skewed, local), Some(Duration::from_secs(10)));
        let past = headers(&[(RETRY_AFTER, "Wed, 14 Oct 2026 11:59:00 GMT")]);
        assert_eq!(retry_after(&past, local), Some(Duration::ZERO));
        // The obsolete RFC 850 and asctime forms are HTTP dates too.
        let asctime = headers(&[(RETRY_AFTER, "Wed Oct 14 12:01:00 2026")]);
        assert_eq!(retry_after(&asctime, local), Some(Duration::from_secs(60)));
    }

    #[test]
    fn retry_after_ignores_garbage() {
        let now = at("Wed, 14 Oct 2026 12:00:00 GMT");
        for value in [
            "",
            "soon",
            "-5",
            "1.5",
            "12s",
            "Wed, 99 Oct 2026 12:00:00 GMT",
        ] {
            assert_eq!(
                retry_after(&headers(&[(RETRY_AFTER, value)]), now),
                None,
                "{value:?}"
            );
        }
        assert_eq!(retry_after(&HeaderMap::new(), now), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_with_equal_jitter() {
        let policy = RateLimitPolicy::default();
        assert_eq!(policy.backoff(1, 0.0), Duration::from_millis(500));
        assert_eq!(policy.backoff(1, 1.0), Duration::from_secs(1));
        assert_eq!(policy.backoff(2, 1.0), Duration::from_secs(2));
        assert_eq!(policy.backoff(3, 0.5), Duration::from_secs(3));
        assert_eq!(policy.backoff(6, 1.0), Duration::from_secs(30));
        assert_eq!(policy.backoff(u32::MAX, 1.0), Duration::from_secs(30));
        assert_eq!(policy.backoff(u32::MAX, 0.0), Duration::from_secs(15));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_sets_the_cool_down_and_is_capped() {
        let limiter = RateLimiter::default();
        let segment = url("https://cdn.example/live/1.m4s");
        let delay = limiter.observe(
            &segment,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "7")]),
        );
        assert_eq!(delay, Some(Duration::from_secs(7)));

        let other = url("https://other.example/live/1.m4s");
        let delay = limiter.observe(
            &other,
            StatusCode::SERVICE_UNAVAILABLE,
            &headers(&[(RETRY_AFTER, "86400")]),
        );
        assert_eq!(delay, Some(Duration::from_secs(60)), "capped");

        // A 503 without Retry-After is a server error, not rate limiting.
        let third = url("https://third.example/a.mpd");
        assert_eq!(
            limiter.observe(&third, StatusCode::SERVICE_UNAVAILABLE, &HeaderMap::new()),
            None
        );
        let error =
            DownloadError::http_status(StatusCode::SERVICE_UNAVAILABLE, third.as_str(), "t");
        assert_eq!(limiter.throttled(&third, &error), None);
    }

    #[tokio::test(start_paused = true)]
    async fn requests_wait_for_the_cool_down_and_one_probe() {
        let limiter = Arc::new(RateLimiter::default());
        let cancel = CancellationToken::new();
        let segment = url("https://cdn.example/live/1.m4s");
        let started = Instant::now();

        // Other origins are not affected.
        limiter.observe(
            &segment,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "10")]),
        );
        let elsewhere = limiter
            .admit(&url("https://other.example/a"), &cancel)
            .await
            .unwrap();
        assert!(elsewhere.probe.is_none());
        assert_eq!(started.elapsed(), Duration::ZERO);

        let probe = limiter.admit(&segment, &cancel).await.unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(10));
        assert!(probe.probe.is_some());

        // While the probe is out, everyone else waits for its answer.
        let waiter = tokio::spawn({
            let limiter = Arc::clone(&limiter);
            let cancel = cancel.clone();
            let segment = segment.clone();
            async move {
                let admission = limiter.admit(&segment, &cancel).await.unwrap();
                (Instant::now(), admission.probe.is_some())
            }
        });
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!waiter.is_finished());

        // The probe succeeds: the cool-down is over for everyone.
        assert_eq!(
            limiter.observe(&segment, StatusCode::OK, &HeaderMap::new()),
            None
        );
        drop(probe);
        let (admitted_at, was_probe) = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter released")
            .unwrap();
        assert_eq!(admitted_at - started, Duration::from_secs(15));
        assert!(!was_probe, "no cool-down left, so no probe");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_probe_backs_off_further_and_a_lost_probe_is_handed_on() {
        let limiter = Arc::new(RateLimiter::new(RateLimitPolicy {
            base_backoff: Duration::from_secs(2),
            ..RateLimitPolicy::default()
        }));
        let cancel = CancellationToken::new();
        let mpd = url("https://origin.example/live.mpd");
        let started = Instant::now();

        let first = limiter
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert!((Duration::from_secs(1)..=Duration::from_secs(2)).contains(&first));

        // Concurrent answers to requests sent before the cool-down do not
        // escalate it.
        let concurrent = limiter
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert!(concurrent <= first);
        // Nor does an earlier success end it.
        limiter.observe(&mpd, StatusCode::OK, &HeaderMap::new());
        let error = DownloadError::http_status(StatusCode::TOO_MANY_REQUESTS, mpd.as_str(), "t");
        assert!(limiter.throttled(&mpd, &error).is_some());

        // The timer has millisecond resolution.
        let around = |expected: Duration| {
            let elapsed = started.elapsed();
            assert!(
                elapsed >= expected && elapsed <= expected + Duration::from_millis(5),
                "{elapsed:?} vs {expected:?}"
            );
        };
        let probe = limiter.admit(&mpd, &cancel).await.unwrap();
        around(first);
        let second = limiter
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert!(
            (Duration::from_secs(2)..=Duration::from_secs(4)).contains(&second),
            "{second:?}"
        );
        drop(probe);

        // A probe that never answers (network error) hands probing on.
        let lost = limiter.admit(&mpd, &cancel).await.unwrap();
        around(first + second);
        let next = tokio::spawn({
            let limiter = Arc::clone(&limiter);
            let cancel = cancel.clone();
            async move {
                let admission = limiter.admit(&mpd, &cancel).await.unwrap();
                admission.probe.is_some()
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!next.is_finished(), "waits for the outstanding probe");
        drop(lost);
        let was_probe = tokio::time::timeout(Duration::from_millis(10), next)
            .await
            .expect("probe handed on")
            .unwrap();
        assert!(was_probe);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_ends_on_cancellation() {
        let limiter = RateLimiter::default();
        let cancel = CancellationToken::new();
        let mpd = url("https://origin.example/live.mpd");
        limiter.observe(
            &mpd,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "60")]),
        );
        let waiting = limiter.admit(&mpd, &cancel);
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("cancellation ends the wait");
        assert!(matches!(result, Err(DownloadError::Cancelled)));
    }
}
