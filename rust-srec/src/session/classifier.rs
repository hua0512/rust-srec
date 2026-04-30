//! Per-engine classifier — turns a `Terminal::Failed` into an
//! [`OfflineSignal`] when the engine-side error is strong enough to be a
//! definitive offline signal that bypasses the slower hysteresis path.
//!
//! ## Rules
//!
//! - **mesio HLS / mesio FLV + `Network` failures** — accumulate per
//!   streamer; reaching the threshold inside the trailing window is treated
//!   as a definitive offline. Returns
//!   [`OfflineSignal::ConsecutiveFailures(threshold)`]. Counter resets when a
//!   successful segment is observed (preserves Bilibili-style mid-stream
//!   RST reconnects).
//! - **ffmpeg / streamlink** — subprocess errors are too fuzzy to
//!   classify with high confidence. Returns `None`; the slower monitor
//!   path observes offline on the next successful status check.
//! - **Everything else** — `None`. Includes `HttpClientError` (4xx/404 — see
//!   note below), `SourceUnavailable` (the engine already gave up),
//!   `Processing` (writer error), `Io`, `OutputRootUnavailable`
//!   (infrastructure), `Configuration`, `RateLimited`, server 5xx, etc.
//!
//! ## Why HTTP 404 is not classified
//!
//! Earlier revisions promoted any mesio 404 to `DefinitiveOffline`. That
//! over-fired in two real production cases:
//!
//! 1. **FLV initial-request 404 on stream re-up** — Douyu and similar CDNs
//!    occasionally return 404 on the freshly-issued FLV URL while the edge
//!    propagates the new token. The platform monitor still reports `LIVE`,
//!    so the only effect of the early end was three back-to-back empty
//!    sessions (one per `LIVE` re-detection) before the actor's error
//!    backoff finally engaged.
//! 2. **HLS segment / playlist 404 mid-stream** — sliding-window eviction
//!    races, signed-URL token expiry on platforms that 404 instead of 403,
//!    and CDN edge desync all manifest as transient 404s on a stream that
//!    is otherwise live.
//!
//! True end-of-stream signals are now sourced from:
//! - Mesio HLS `#EXT-X-ENDLIST` → `EngineEndSignal::HlsEndlist`
//!   (authoritative, bypasses hysteresis directly via the `Completed` path)
//! - The consecutive-`Network` rule below, which catches genuinely-offline
//!   streams without misfiring on transient 404s.
//!
//! ## Configuration
//!
//! Window and threshold are **derived from the scheduler's existing
//! `offline_check_*` tunables** so operators only configure offline
//! detection in one place. See [`OfflineClassifier::from_scheduler`].
//!
//! The classifier is stateful (holds the per-streamer consecutive-failure
//! window). Constructed once per process and shared with
//! [`crate::session::SessionLifecycle`].

use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, info};

use crate::downloader::DownloadFailureKind;
use crate::session::state::OfflineSignal;

/// Floor for the classifier threshold. Operators who set
/// `offline_check_count = 1` for very aggressive offline polling still get a
/// safety margin against mid-stream RST reconnects, which the
/// `note_successful_segment` reset covers but only after the segment lands.
const MIN_CONSECUTIVE_FAILURE_THRESHOLD: usize = 2;

/// A classifier inspects an engine failure and decides whether it is a
/// definitive offline signal worth bypassing the slower hysteresis path.
#[derive(Debug)]
pub struct OfflineClassifier {
    /// Per-streamer timestamps of recent eligible (Network) failures.
    /// Pruned to the trailing [`Self::window`] on each update.
    failure_log: DashMap<String, Vec<Instant>>,
    /// Trailing window for the consecutive-failures rule.
    window: Duration,
    /// Number of failures inside [`Self::window`] that trips the
    /// definitive-offline signal. Floored at
    /// [`MIN_CONSECUTIVE_FAILURE_THRESHOLD`].
    threshold: usize,
}

impl OfflineClassifier {
    /// Construct a classifier with the [`Default`] window/threshold,
    /// matching `SchedulerConfig::default`. Provided as a thin shim for
    /// call sites that don't have the scheduler config in scope (mostly
    /// tests and the legacy monitor service entry point).
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a classifier from the scheduler's existing offline-check
    /// tunables — single source of truth for "how long do we wait before
    /// declaring a stream really offline" across the
    /// [`crate::session::HysteresisConfig`] backstop and this classifier.
    ///
    /// Window = `count × interval_ms` (matches the actor's offline
    /// confirmation horizon). Threshold = `max(count, 2)` — a floor of 2
    /// preserves Bilibili-RST safety even when an operator dials
    /// `offline_check_count = 1` for very aggressive polling.
    pub fn from_scheduler(offline_check_count: u32, offline_check_interval_ms: u64) -> Self {
        let count = offline_check_count.max(1);
        let interval = offline_check_interval_ms.max(1_000);
        let window = Duration::from_millis(count as u64 * interval);
        let threshold = (count as usize).max(MIN_CONSECUTIVE_FAILURE_THRESHOLD);
        Self {
            failure_log: DashMap::new(),
            window,
            threshold,
        }
    }

    /// Construct directly from explicit window/threshold values. Used by
    /// the test-only [`OfflineClassifier::new`] / [`Default`] path so test
    /// fixtures get the historical `(60 s, 2)` defaults regardless of
    /// changes to [`SchedulerConfig::default`]. Production code goes
    /// through [`Self::from_scheduler`].
    pub fn from_window_threshold(window: Duration, threshold: usize) -> Self {
        Self {
            failure_log: DashMap::new(),
            window,
            threshold: threshold.max(MIN_CONSECUTIVE_FAILURE_THRESHOLD),
        }
    }

    /// Trailing window — primarily for test assertions on the prune
    /// boundary.
    #[cfg(test)]
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Classify a terminal failure against the streamer's recent history.
    ///
    /// Returns `Some(signal)` only when the failure is a high-confidence
    /// offline signal that should bypass the slower hysteresis path.
    pub fn classify_failure(
        &self,
        streamer_id: &str,
        engine_kind: &EngineKind,
        failure: &DownloadFailureKind,
    ) -> Option<OfflineSignal> {
        // ffmpeg / streamlink: never classify (subprocess noise).
        if !engine_kind.is_mesio() {
            debug!(
                streamer_id,
                engine = ?engine_kind,
                failure = ?failure,
                "OfflineClassifier: non-mesio engine, no signal"
            );
            return None;
        }

        match failure {
            DownloadFailureKind::Network => self.record_network_failure_and_check(streamer_id),
            _ => {
                debug!(
                    streamer_id,
                    engine = ?engine_kind,
                    failure = ?failure,
                    "OfflineClassifier: non-matching failure, no signal"
                );
                None
            }
        }
    }

    /// Reset the per-streamer consecutive-failures counter when a
    /// successful segment is observed.
    pub fn note_successful_segment(&self, streamer_id: &str) {
        if self.failure_log.remove(streamer_id).is_some() {
            debug!(
                streamer_id,
                "OfflineClassifier: successful segment cleared pending-failure log"
            );
        }
    }

    /// Append a Network failure to the per-streamer log and decide if the
    /// trailing-window threshold has been reached.
    fn record_network_failure_and_check(&self, streamer_id: &str) -> Option<OfflineSignal> {
        let now = Instant::now();
        let window = self.window;
        let threshold = self.threshold;
        let mut entry = self.failure_log.entry(streamer_id.to_string()).or_default();
        let log = entry.value_mut();

        // Prune anything outside the window before appending.
        log.retain(|t| now.duration_since(*t) < window);
        log.push(now);
        let count = log.len();

        if count >= threshold {
            // Clear after firing so the next N failures inside the next
            // window trip again (don't hold the counter at threshold).
            log.clear();
            info!(
                streamer_id,
                count,
                threshold,
                window_secs = window.as_secs(),
                "OfflineClassifier: consecutive Network failures → DefinitiveOffline(ConsecutiveFailures)"
            );
            Some(OfflineSignal::ConsecutiveFailures(threshold as u32))
        } else {
            debug!(
                streamer_id,
                count,
                threshold,
                window_secs = window.as_secs(),
                "OfflineClassifier: Network failure logged; below threshold"
            );
            None
        }
    }
}

impl Default for OfflineClassifier {
    /// Default uses the historical fixed `(60 s window, threshold 2)`
    /// behaviour. Provided so test fixtures (`make_lifecycle`,
    /// integration helpers) don't depend on the scheduler-default drift
    /// — production wires explicitly through [`Self::from_scheduler`].
    fn default() -> Self {
        Self::from_window_threshold(Duration::from_secs(60), 2)
    }
}

/// Which download engine produced the failure. Distinguishing engines is
/// required because mesio failures can be classified, but `ffmpeg`
/// subprocess errors cannot.
///
/// Mirrors the variants of `crate::downloader::engine::EngineType` with the
/// mesio flavour split out (`HLS` vs `FLV`) because the rules are the same
/// for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    MesioHls,
    MesioFlv,
    Ffmpeg,
    Streamlink,
}

impl EngineKind {
    pub fn is_mesio(&self) -> bool {
        matches!(self, Self::MesioHls | Self::MesioFlv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: the test-default classifier (`60 s window, threshold 2`),
    /// matching the historical hardcoded constants so existing assertions
    /// stay numerically stable. Equivalent to `OfflineClassifier::new()` /
    /// `Default::default()`.
    fn classic_classifier() -> OfflineClassifier {
        OfflineClassifier::new()
    }

    // ---- C2 — single Network failure does not classify ----------------

    #[test]
    fn c2_single_network_failure_does_not_classify() {
        let c = classic_classifier();
        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::Network,
        );
        assert_eq!(result, None);
    }

    // ---- C3 — two consecutive Network failures within window → Some ----

    #[test]
    fn c3_two_consecutive_network_failures_classify_as_definitive_offline() {
        let c = classic_classifier();

        let first = c.classify_failure(
            "s1",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(first, None, "first Network alone must not classify");

        let second = c.classify_failure(
            "s1",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(
            second,
            Some(OfflineSignal::ConsecutiveFailures(2)),
            "second Network inside window must classify"
        );
    }

    // ---- C4 — window expiry resets the counter ------------------------

    #[test]
    fn c4_expired_window_resets_counter() {
        let c = classic_classifier();

        // Manually seed the log with a timestamp just past the window so we
        // don't have to block the test for a minute.
        let stale = Instant::now()
            .checked_sub(c.window() + Duration::from_secs(1))
            .expect("stale timestamp");
        c.failure_log.insert("s1".to_string(), vec![stale]);

        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::Network,
        );
        assert_eq!(
            result, None,
            "stale entries must be pruned before threshold check"
        );
    }

    // ---- C5 — successful segment resets the counter -------------------

    #[test]
    fn c5_successful_segment_resets_counter() {
        let c = classic_classifier();

        // First failure primes the counter.
        let first = c.classify_failure(
            "s1",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(first, None);

        // Successful segment clears the log.
        c.note_successful_segment("s1");

        // Next failure must be treated as the first again (not the second).
        let after = c.classify_failure(
            "s1",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(
            after, None,
            "counter must reset after successful segment"
        );
    }

    // ---- C6 / C7 — ffmpeg / streamlink subprocess errors → None -------

    #[test]
    fn c6_ffmpeg_http_404_does_not_classify() {
        let c = classic_classifier();
        let result = c.classify_failure(
            "s1",
            &EngineKind::Ffmpeg,
            &DownloadFailureKind::HttpClientError { status: 404 },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c6_ffmpeg_subprocess_error_is_none() {
        let c = classic_classifier();
        let result = c.classify_failure(
            "s1",
            &EngineKind::Ffmpeg,
            &DownloadFailureKind::ProcessExit { code: Some(1) },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c7_streamlink_subprocess_error_is_none() {
        let c = classic_classifier();
        let result = c.classify_failure(
            "s1",
            &EngineKind::Streamlink,
            &DownloadFailureKind::ProcessExit { code: Some(2) },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c7_streamlink_network_never_accumulates() {
        let c = classic_classifier();
        // Accumulate many Network failures on a streamlink engine; counter
        // should never fire because streamlink failures are not classified.
        for _ in 0..5 {
            let result = c.classify_failure(
                "s1",
                &EngineKind::Streamlink,
                &DownloadFailureKind::Network,
            );
            assert_eq!(result, None);
        }
    }

    // ---- Additional coverage ------------------------------------------

    /// Mesio 404 (the previously-classified case) now returns `None` — the
    /// FLV initial-request CDN race is not a definitive offline signal.
    #[test]
    fn mesio_404_no_longer_classifies() {
        let c = classic_classifier();
        assert_eq!(
            c.classify_failure(
                "s1",
                &EngineKind::MesioHls,
                &DownloadFailureKind::HttpClientError { status: 404 },
            ),
            None
        );
        assert_eq!(
            c.classify_failure(
                "s1",
                &EngineKind::MesioFlv,
                &DownloadFailureKind::HttpClientError { status: 404 },
            ),
            None
        );
    }

    /// Non-network, non-404 mesio failures return `None`.
    #[test]
    fn mesio_source_unavailable_returns_none() {
        let c = classic_classifier();
        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::SourceUnavailable,
        );
        assert_eq!(result, None);
    }

    /// HTTP 5xx and other client errors are not definitive.
    #[test]
    fn mesio_500_and_403_return_none() {
        let c = classic_classifier();
        assert_eq!(
            c.classify_failure(
                "s1",
                &EngineKind::MesioHls,
                &DownloadFailureKind::HttpServerError { status: 500 }
            ),
            None
        );
        assert_eq!(
            c.classify_failure(
                "s1",
                &EngineKind::MesioHls,
                &DownloadFailureKind::HttpClientError { status: 403 }
            ),
            None
        );
    }

    /// Per-streamer isolation: streamer A's failures don't trip streamer B's
    /// counter.
    #[test]
    fn streamer_isolation_in_consecutive_counter() {
        let c = classic_classifier();

        let a1 = c.classify_failure(
            "a",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        let b1 = c.classify_failure(
            "b",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(a1, None);
        assert_eq!(b1, None);

        // A second Network on A fires; B's log is still at 1.
        let a2 = c.classify_failure(
            "a",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(a2, Some(OfflineSignal::ConsecutiveFailures(2)));

        let b_still = c.classify_failure(
            "b",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::Network,
        );
        assert_eq!(
            b_still,
            Some(OfflineSignal::ConsecutiveFailures(2)),
            "streamer b's counter is independent of a's firing"
        );
    }

    // ---- from_scheduler / threshold floor ------------------------------

    #[test]
    fn from_scheduler_default_matches_60s_window_threshold_3() {
        // With offline_check_count=3, threshold derives to 3 (count, since
        // count > MIN floor of 2). Window = 3 × 20s = 60s.
        let c = OfflineClassifier::from_scheduler(3, 20_000);
        assert_eq!(c.window(), Duration::from_secs(60));
        assert_eq!(c.threshold, 3);
    }

    #[test]
    fn from_scheduler_threshold_floor_of_two() {
        // count=1 must floor to 2 to retain Bilibili-RST safety.
        let c = OfflineClassifier::from_scheduler(1, 30_000);
        assert_eq!(c.threshold, 2);
        assert_eq!(c.window(), Duration::from_secs(30));
    }

    #[test]
    fn from_scheduler_higher_count_widens_window() {
        // count=5 → threshold=5, window = 5 × 10s = 50s. Two Network
        // failures alone are NOT enough.
        let c = OfflineClassifier::from_scheduler(5, 10_000);
        assert_eq!(c.threshold, 5);

        for _ in 0..4 {
            assert_eq!(
                c.classify_failure(
                    "s",
                    &EngineKind::MesioFlv,
                    &DownloadFailureKind::Network
                ),
                None
            );
        }
        assert_eq!(
            c.classify_failure(
                "s",
                &EngineKind::MesioFlv,
                &DownloadFailureKind::Network
            ),
            Some(OfflineSignal::ConsecutiveFailures(5))
        );
    }
}
