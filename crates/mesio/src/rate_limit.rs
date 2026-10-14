//! Back-pressure for servers that rate limit, shared by every request made
//! through one [`ClientPool`](crate::downloader::ClientPool).
//!
//! A `429 Too Many Requests`, or a `503 Service Unavailable` that carries a
//! `Retry-After` header, puts origins into a cool-down: the origin the
//! request was sent to, and the origin that answered when a redirect led
//! elsewhere. Requests are admitted by the origin they are sent to, so a
//! refusal from a redirect target pauses the requests that lead there,
//! while direct requests to the target are paused by its own cool-down.
//! While it lasts, manifest, playlist and Segment Index requests wait for
//! it ([`RateLimiter::admit`]), while segment and key requests return at
//! once ([`RateLimiter::try_admit`]) so that a waiting segment does not hold
//! a download slot that requests to other servers could use.
//!
//! When the cool-down ends, one request goes first as a probe and the
//! others wait for its answer, so a server that is recovering does not
//! receive the whole backlog at once. Only an answer to a request admitted
//! after the cool-down began can end it or extend it with more backoff;
//! answers to older requests say nothing about the server since.
//!
//! The cool-down lasts as long as `Retry-After` asks (delay-seconds or an
//! HTTP date), between [`RateLimitPolicy::min_cool_down`] and
//! [`RateLimitPolicy::max_retry_after`]. Without a usable `Retry-After`, it
//! backs off exponentially from [`RateLimitPolicy::base_backoff`], doubling
//! for each probe that is rate limited again up to
//! [`RateLimitPolicy::max_backoff`], with equal jitter.
//!
//! Rate limiting is not counted against the retry budgets of segments and
//! manifest refreshes, which are meant for failures that waiting does not
//! cure. It is bounded by [`RateLimitPolicy::max_throttled`] instead: a
//! segment fails once that long has passed since it was first held back, a
//! manifest or playlist once that long has passed without a successful
//! refresh, waiting for admission included.

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

/// How long a request held back by an outstanding probe waits before it
/// tries again. Waiting requests are also woken by every state change.
const PROBE_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateLimitPolicy {
    /// First cool-down without a usable `Retry-After`.
    pub base_backoff: Duration,
    /// Longest cool-down without a usable `Retry-After`.
    pub max_backoff: Duration,
    /// Shortest cool-down, whatever `Retry-After` says: a server answering
    /// `Retry-After: 0` (or a past date) to every request would otherwise be
    /// asked again in a tight loop.
    pub min_cool_down: Duration,
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
            min_cool_down: Duration::from_millis(250),
            max_retry_after: Duration::from_secs(60),
            max_throttled: Duration::from_secs(5 * 60),
        }
    }
}

impl RateLimitPolicy {
    /// What is left of the allowance for rate limiting that began at
    /// `since`; `None` once it is spent. Delays are clamped to it, so the
    /// bound holds even when a long cool-down starts near its end.
    pub(crate) fn allowance_left(&self, since: Instant) -> Option<Duration> {
        self.max_throttled
            .checked_sub(since.elapsed())
            .filter(|left| !left.is_zero())
    }

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

/// Whether `status` asks the client to slow down. A `503` is rate limiting
/// when it carries `Retry-After` at all, even one that cannot be parsed
/// (the cool-down then backs off as if there were none); without the
/// header it is an ordinary server error.
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

/// A request held back because its origin is cooling down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Throttled {
    /// The status that started the cool-down.
    pub status: StatusCode,
    /// When to try again.
    pub retry_in: Duration,
}

impl Throttled {
    pub(crate) fn error(self, url: &Url, operation: &'static str) -> DownloadError {
        DownloadError::rate_limited(self.status, url.as_str(), operation, self.retry_in)
    }
}

#[derive(Debug)]
struct Cooldown {
    /// Consecutive rate-limited answers to fresh requests; what the backoff
    /// grows with.
    strikes: u32,
    until: Instant,
    /// The admission sequence number when the cool-down began. Requests
    /// admitted before it were sent before the server asked for a pause.
    started: u64,
    /// The admission sequence number of the probe sent after `until`, while
    /// its answer is pending.
    probe: Option<u64>,
    status: StatusCode,
}

#[derive(Debug, Default)]
struct State {
    /// By origin (scheme, host and port).
    cooldowns: HashMap<String, Cooldown>,
    /// Last admission sequence number handed out.
    seq: u64,
}

impl State {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
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
/// passed to [`Self::observe`]. A probe dropped without an answer (a
/// network error, a timeout, cancellation) hands probing to the next
/// waiting request.
#[must_use = "a probe is released as soon as its admission is dropped"]
pub(crate) struct Admission<'a> {
    limiter: &'a RateLimiter,
    seq: u64,
    /// The origin the request is sent to.
    origin: String,
    /// Whether this request is that origin's probe.
    probe: bool,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if self.probe {
            self.limiter.release_probe(&self.origin, self.seq);
        }
    }
}

impl Admission<'_> {
    /// Records the answer to the admitted request. `answered` is the URL
    /// that answered (`Response::url`, after redirects). Returns how long
    /// the request's origin now cools down when the answer is rate
    /// limiting.
    pub(crate) fn observe(
        &self,
        answered: &Url,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Duration> {
        self.limiter.observe(self, answered, status, headers)
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

    /// Admits a request to `url` at once, or says how long its origin is
    /// still cooling down. For requests that should give their resources
    /// back rather than wait.
    pub(crate) fn try_admit(&self, url: &Url) -> Result<Admission<'_>, Throttled> {
        let origin = origin_key(url);
        let mut state = self.state.lock();
        let seq = state.next_seq();
        let Some(cooldown) = state.cooldowns.get_mut(&origin) else {
            return Ok(Admission {
                limiter: self,
                seq,
                origin,
                probe: false,
            });
        };
        let now = Instant::now();
        if now < cooldown.until {
            return Err(Throttled {
                status: cooldown.status,
                retry_in: cooldown.until - now,
            });
        }
        if cooldown.probe.is_some() {
            return Err(Throttled {
                status: cooldown.status,
                retry_in: PROBE_POLL,
            });
        }
        cooldown.probe = Some(seq);
        debug!(origin, "rate limit cool-down over; probing");
        Ok(Admission {
            limiter: self,
            seq,
            origin,
            probe: true,
        })
    }

    /// Waits until a request to `url` may be sent. Fails when `cancel`
    /// fires, or with [`DownloadError::RateLimited`] once `deadline` passes
    /// while the origin is still cooling down.
    pub(crate) async fn admit(
        &self,
        url: &Url,
        cancel: &CancellationToken,
        deadline: Instant,
        operation: &'static str,
    ) -> Result<Admission<'_>, DownloadError> {
        loop {
            // Registered before the state is read, so a change between the
            // read and the wait still wakes this request.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let throttled = match self.try_admit(url) {
                Ok(admission) => return Ok(admission),
                Err(throttled) => throttled,
            };
            let now = Instant::now();
            if now >= deadline {
                return Err(throttled.error(url, operation));
            }
            let wake_at = (now + throttled.retry_in).min(deadline);
            tokio::select! {
                () = cancel.cancelled() => return Err(DownloadError::Cancelled),
                () = &mut notified => {}
                () = tokio::time::sleep_until(wake_at) => {}
            }
        }
    }

    fn observe(
        &self,
        admission: &Admission<'_>,
        answered: &Url,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Duration> {
        let has_retry_after = headers.contains_key(RETRY_AFTER);
        let retry_after = retry_after(headers, Utc::now());
        let answered = origin_key(answered);
        let mut origins = vec![admission.origin.as_str()];
        if answered != admission.origin {
            origins.push(&answered);
        }
        let now = Instant::now();
        let mut state = self.state.lock();
        if !is_rate_limited(status, has_retry_after) {
            let mut ended = false;
            for origin in origins {
                if state
                    .cooldowns
                    .get(origin)
                    .is_some_and(|cooldown| admission.seq > cooldown.started)
                {
                    state.cooldowns.remove(origin);
                    ended = true;
                    debug!(origin, "server stopped rate limiting");
                }
            }
            drop(state);
            if ended {
                self.changed.notify_waiters();
            }
            return None;
        }

        let wait = retry_after
            .map(|wait| wait.clamp(self.policy.min_cool_down, self.policy.max_retry_after));
        let mut requested_wait = Duration::ZERO;
        for origin in origins {
            let remaining = self.cool_down(&mut state, origin, admission.seq, status, wait, now);
            if origin == admission.origin {
                requested_wait = remaining;
            }
        }
        drop(state);
        self.changed.notify_waiters();
        Some(requested_wait)
    }

    /// Puts `origin` into a cool-down after a rate-limiting answer to the
    /// request admitted as `seq`, and returns what is left of it.
    fn cool_down(
        &self,
        state: &mut State,
        origin: &str,
        seq: u64,
        status: StatusCode,
        wait: Option<Duration>,
        now: Instant,
    ) -> Duration {
        let strikes = match state.cooldowns.get_mut(origin) {
            // Sent before this cool-down began: no evidence that the server
            // needs longer, unless it says so.
            Some(cooldown) if seq < cooldown.started => {
                if let Some(wait) = wait {
                    cooldown.until = cooldown.until.max(now + wait);
                }
                return cooldown.until.saturating_duration_since(now);
            }
            Some(cooldown) => cooldown.strikes.saturating_add(1),
            None => 1,
        };
        let delay = wait.unwrap_or_else(|| {
            self.policy
                .backoff(strikes, rand::rng().random_range(0.0..=1.0))
                .max(self.policy.min_cool_down)
        });
        let started = state.next_seq();
        state.cooldowns.insert(
            origin.to_owned(),
            Cooldown {
                strikes,
                until: now + delay,
                started,
                probe: None,
                status,
            },
        );
        warn!(
            origin,
            status = status.as_u16(),
            strikes,
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "server is rate limiting; pausing requests to it"
        );
        delay
    }

    fn release_probe(&self, origin: &str, seq: u64) {
        let mut state = self.state.lock();
        if let Some(cooldown) = state.cooldowns.get_mut(origin)
            && cooldown.probe == Some(seq)
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

    const NEVER: Duration = Duration::from_secs(24 * 60 * 60);

    fn far() -> Instant {
        Instant::now() + NEVER
    }

    /// Admits and answers one request to `url` at once.
    fn answer(
        limiter: &RateLimiter,
        url: &Url,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Duration> {
        let admission = limiter.try_admit(url).expect("admitted");
        admission.observe(url, status, headers)
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
    async fn retry_after_sets_the_cool_down_within_its_bounds() {
        let limiter = RateLimiter::default();
        let segment = url("https://cdn.example/live/1.m4s");
        let delay = answer(
            &limiter,
            &segment,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "7")]),
        );
        assert_eq!(delay, Some(Duration::from_secs(7)));

        let other = url("https://other.example/live/1.m4s");
        let delay = answer(
            &limiter,
            &other,
            StatusCode::SERVICE_UNAVAILABLE,
            &headers(&[(RETRY_AFTER, "86400")]),
        );
        assert_eq!(delay, Some(Duration::from_secs(60)), "capped");

        // An immediate retry is still a pause: the server would otherwise be
        // asked again in a tight loop.
        let eager = url("https://eager.example/a.m4s");
        let delay = answer(
            &limiter,
            &eager,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "0")]),
        );
        assert_eq!(delay, Some(Duration::from_millis(250)));

        // A 503 without Retry-After is a server error, not rate limiting.
        let third = url("https://third.example/a.mpd");
        assert_eq!(
            answer(
                &limiter,
                &third,
                StatusCode::SERVICE_UNAVAILABLE,
                &HeaderMap::new()
            ),
            None
        );
        assert!(limiter.try_admit(&third).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_503_with_an_unusable_retry_after_backs_off() {
        let limiter = RateLimiter::default();
        let mpd = url("https://origin.example/live.mpd");
        let delay = answer(
            &limiter,
            &mpd,
            StatusCode::SERVICE_UNAVAILABLE,
            &headers(&[(RETRY_AFTER, "in a while")]),
        )
        .expect("rate limited");
        assert!(
            (Duration::from_millis(500)..=Duration::from_secs(1)).contains(&delay),
            "{delay:?}"
        );
        let throttled = limiter.try_admit(&mpd).err().expect("cooling down");
        assert_eq!(throttled.status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test(start_paused = true)]
    async fn requests_wait_for_the_cool_down_and_one_probe() {
        let limiter = Arc::new(RateLimiter::default());
        let cancel = CancellationToken::new();
        let segment = url("https://cdn.example/live/1.m4s");
        let started = Instant::now();

        answer(
            &limiter,
            &segment,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "10")]),
        );
        // Other origins are not affected.
        let elsewhere = limiter
            .admit(&url("https://other.example/a"), &cancel, far(), "t")
            .await
            .unwrap();
        assert!(!elsewhere.probe);
        assert_eq!(started.elapsed(), Duration::ZERO);
        // Requests that must not wait are told how long to stay away.
        assert_eq!(
            limiter.try_admit(&segment).err(),
            Some(Throttled {
                status: StatusCode::TOO_MANY_REQUESTS,
                retry_in: Duration::from_secs(10),
            })
        );

        let probe = limiter.admit(&segment, &cancel, far(), "t").await.unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(10));
        assert!(probe.probe);
        assert_eq!(
            limiter.try_admit(&segment).err().map(|t| t.retry_in),
            Some(PROBE_POLL),
            "the probe is out"
        );

        // While the probe is out, everyone else waits for its answer.
        let waiter = tokio::spawn({
            let limiter = Arc::clone(&limiter);
            let cancel = cancel.clone();
            let segment = segment.clone();
            async move {
                let admission = limiter.admit(&segment, &cancel, far(), "t").await.unwrap();
                (Instant::now(), admission.probe)
            }
        });
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!waiter.is_finished());

        // The probe succeeds: the cool-down is over for everyone.
        assert_eq!(
            probe.observe(&segment, StatusCode::OK, &HeaderMap::new()),
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
    async fn only_answers_to_requests_admitted_since_the_cool_down_began_count() {
        let limiter = RateLimiter::new(RateLimitPolicy {
            base_backoff: Duration::from_secs(2),
            ..RateLimitPolicy::default()
        });
        let mpd = url("https://origin.example/live.mpd");
        // Three requests are out when the server starts refusing.
        let refused = limiter.try_admit(&mpd).unwrap();
        let slow = limiter.try_admit(&mpd).unwrap();
        let late_refusal = limiter.try_admit(&mpd).unwrap();
        let first = refused
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert!((Duration::from_secs(1)..=Duration::from_secs(2)).contains(&first));

        // A refusal of an older request does not escalate the backoff.
        let concurrent = late_refusal
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert_eq!(concurrent, first);

        // The cool-down ends and a probe goes out. The older request's
        // success arrives only now: it must neither end the cool-down nor
        // release the requests waiting for the probe.
        tokio::time::sleep(first).await;
        let probe = limiter.try_admit(&mpd).expect("probe");
        assert!(probe.probe);
        assert_eq!(slow.observe(&mpd, StatusCode::OK, &HeaderMap::new()), None);
        assert!(
            limiter.try_admit(&mpd).is_err(),
            "still waiting for the probe"
        );

        // The probe is refused again: the backoff grows.
        let second = probe
            .observe(&mpd, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
            .unwrap();
        assert!(
            (Duration::from_secs(2)..=Duration::from_secs(4)).contains(&second),
            "{second:?}"
        );
        drop(probe);
        tokio::time::sleep(second).await;
        let probe = limiter.try_admit(&mpd).expect("next probe");
        probe.observe(&mpd, StatusCode::OK, &HeaderMap::new());
        drop(probe);
        assert!(!limiter.try_admit(&mpd).unwrap().probe, "over");
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_probe_is_handed_on() {
        let limiter = Arc::new(RateLimiter::default());
        let cancel = CancellationToken::new();
        let mpd = url("https://origin.example/live.mpd");
        answer(
            &limiter,
            &mpd,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "1")]),
        );
        let lost = limiter.admit(&mpd, &cancel, far(), "t").await.unwrap();
        let next = tokio::spawn({
            let limiter = Arc::clone(&limiter);
            let cancel = cancel.clone();
            async move {
                let admission = limiter.admit(&mpd, &cancel, far(), "t").await.unwrap();
                admission.probe
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!next.is_finished(), "waits for the outstanding probe");
        // A network error: the probe is dropped without an answer.
        drop(lost);
        let was_probe = tokio::time::timeout(Duration::from_millis(10), next)
            .await
            .expect("probe handed on")
            .unwrap();
        assert!(was_probe);
    }

    #[tokio::test(start_paused = true)]
    async fn admission_waits_end_at_the_deadline() {
        let limiter = Arc::new(RateLimiter::default());
        let cancel = CancellationToken::new();
        let mpd = url("https://origin.example/live.mpd");
        answer(
            &limiter,
            &mpd,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "30")]),
        );
        let started = Instant::now();
        let waiting = limiter.admit(&mpd, &cancel, started + Duration::from_secs(5), "MPD fetch");
        let error = tokio::time::timeout(Duration::from_secs(6), waiting)
            .await
            .expect("bounded by the deadline")
            .err()
            .expect("the cool-down outlasts the deadline");
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        assert!(
            matches!(
                error,
                DownloadError::RateLimited { status, retry_in, operation: "MPD fetch", .. }
                    if status == StatusCode::TOO_MANY_REQUESTS && retry_in == Duration::from_secs(25)
            ),
            "{error:?}"
        );

        // Losing probe races to other requests does not extend the wait
        // past the deadline either.
        tokio::time::sleep(Duration::from_secs(25)).await;
        let racer = limiter.try_admit(&mpd).expect("another request probes");
        let started = Instant::now();
        let waiting = limiter.admit(&mpd, &cancel, started + Duration::from_secs(2), "t");
        let result = tokio::time::timeout(Duration::from_secs(3), waiting)
            .await
            .expect("bounded");
        assert!(matches!(result, Err(DownloadError::RateLimited { .. })));
        assert_eq!(started.elapsed(), Duration::from_secs(2));
        drop(racer);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_ends_on_cancellation() {
        let limiter = RateLimiter::default();
        let cancel = CancellationToken::new();
        let mpd = url("https://origin.example/live.mpd");
        answer(
            &limiter,
            &mpd,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "60")]),
        );
        let waiting = limiter.admit(&mpd, &cancel, far(), "t");
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("cancellation ends the wait");
        assert!(matches!(result, Err(DownloadError::Cancelled)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_redirected_refusal_pauses_the_entry_point_and_the_target() {
        let limiter = RateLimiter::default();
        let playlist = url("https://origin.example/live.m3u8");
        let cdn = url("https://cdn.example/live.m3u8?token=1");
        let elsewhere = url("https://origin.example/other.m3u8");
        // The playlist was redirected to the CDN, which refused it.
        let admission = limiter.try_admit(&playlist).unwrap();
        let delay = admission.observe(
            &cdn,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "5")]),
        );
        assert_eq!(delay, Some(Duration::from_secs(5)));
        drop(admission);
        // Requests that lead there pause, and so do direct ones.
        assert!(
            limiter.try_admit(&elsewhere).is_err(),
            "the entry point pauses"
        );
        assert!(limiter.try_admit(&cdn).is_err(), "the CDN pauses");

        // After the pause, the probe is a request to the entry point; its
        // redirected success ends both cool-downs.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let probe = limiter.try_admit(&playlist).expect("probe");
        assert!(probe.probe);
        assert!(limiter.try_admit(&playlist).is_err(), "one probe at a time");
        assert!(
            probe
                .observe(&cdn, StatusCode::OK, &HeaderMap::new())
                .is_none()
        );
        drop(probe);
        assert!(!limiter.try_admit(&playlist).unwrap().probe);
        assert!(!limiter.try_admit(&cdn).unwrap().probe);
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_redirected_within_its_origin_ends_the_cool_down() {
        let limiter = RateLimiter::default();
        let segment = url("https://origin.example/seg/1.ts");
        let moved = url("https://origin.example/v2/seg/1.ts");
        let admission = limiter.try_admit(&segment).unwrap();
        admission.observe(
            &moved,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[(RETRY_AFTER, "2")]),
        );
        drop(admission);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let probe = limiter.try_admit(&segment).expect("probe");
        assert!(probe.probe);
        probe.observe(&moved, StatusCode::OK, &HeaderMap::new());
        drop(probe);
        assert!(!limiter.try_admit(&segment).unwrap().probe, "recovered");
    }
}
