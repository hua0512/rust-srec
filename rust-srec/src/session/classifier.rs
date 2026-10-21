//! Per-engine classifier — turns a `Terminal::Failed` into an
//! [`OfflineSignal`] when the engine-side error is strong enough to be a
//! definitive offline signal that bypasses the slower hysteresis path.
//!
//! ## Rules
//!
//! - **mesio (HLS, FLV, DASH) + `Network` failures** — accumulate per
//!   streamer; reaching the threshold inside the trailing window is treated
//!   as a definitive offline. Returns
//!   [`OfflineSignal::ConsecutiveFailures`] with the threshold. Counter resets
//!   when a
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
//! HTTP 404 can be transient while a stream remains live: FLV URLs can
//! arrive before a CDN edge has propagated a token, and HLS segment or
//! playlist requests can race sliding-window eviction or token expiry.
//! A 404 alone therefore does not establish `DefinitiveOffline`.
//!
//! Mesio HLS `#EXT-X-ENDLIST` produces `EngineEndSignal::HlsEndlist`, and
//! the end of a static or ended DASH presentation `EngineEndSignal::DashEnd`,
//! through the authoritative `Completed` path. Consecutive `Network`
//! failures use the window and threshold below.
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

use crate::downloader::engine::EngineType;
use crate::downloader::{DownloadFailureKind, DownloadProtocol};
use crate::session::state::OfflineSignal;
use crate::streamer::download_failure_threshold;

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
    /// definitive-offline signal. Uses the shared download-failure floor.
    threshold: usize,
}

impl OfflineClassifier {
    /// Construct a classifier with the [`Default`] 60-second window and
    /// two-failure threshold.
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
        let threshold = download_failure_threshold(count) as usize;
        Self {
            failure_log: DashMap::new(),
            window,
            threshold,
        }
    }

    /// Construct directly from explicit window/threshold values. Used by
    /// the test-only [`OfflineClassifier::new`] / [`Default`] path so test
    /// fixtures get the historical `(60 s, 2)` defaults regardless of
    /// changes to [`SchedulerConfig::default`](crate::scheduler::SchedulerConfig::default).
    /// Production code goes
    /// through [`Self::from_scheduler`].
    pub fn from_window_threshold(window: Duration, threshold: usize) -> Self {
        Self {
            failure_log: DashMap::new(),
            window,
            threshold: threshold.max(download_failure_threshold(1) as usize),
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
/// mesio flavour split out because future rules may differ by protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    MesioHls,
    MesioFlv,
    MesioDash,
    MesioUnknown,
    Ffmpeg,
    Streamlink,
}

impl EngineKind {
    pub fn is_mesio(&self) -> bool {
        matches!(
            self,
            Self::MesioHls | Self::MesioFlv | Self::MesioDash | Self::MesioUnknown
        )
    }

    pub fn from_engine_and_protocol(engine_type: EngineType, protocol: DownloadProtocol) -> Self {
        match engine_type {
            EngineType::Ffmpeg => Self::Ffmpeg,
            EngineType::Streamlink => Self::Streamlink,
            EngineType::Mesio => match protocol {
                DownloadProtocol::Hls => Self::MesioHls,
                DownloadProtocol::Flv => Self::MesioFlv,
                DownloadProtocol::Dash => Self::MesioDash,
                DownloadProtocol::Unknown => Self::MesioUnknown,
            },
        }
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

    #[test]
    fn expired_window_resets_counter() {
        let c = classic_classifier();

        // Manually seed the log with a timestamp just past the window so we
        // don't have to block the test for a minute.
        let stale = Instant::now()
            .checked_sub(c.window() + Duration::from_secs(1))
            .expect("stale timestamp");
        c.failure_log.insert("s1".to_string(), vec![stale]);

        let result = c.classify_failure("s1", &EngineKind::MesioHls, &DownloadFailureKind::Network);
        assert_eq!(
            result, None,
            "stale entries must be pruned before threshold check"
        );
    }

    // ---- Additional coverage ------------------------------------------

    /// Mesio 404 returns `None` — the FLV initial-request CDN race is
    /// not a definitive offline signal.
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

    // ---- from_scheduler / threshold floor ------------------------------

    #[test]
    fn from_scheduler_threshold_floor_of_two() {
        // count=1 must floor to 2 to retain Bilibili-RST safety.
        let c = OfflineClassifier::from_scheduler(1, 30_000);
        assert_eq!(c.threshold, 2);
        assert_eq!(c.window(), Duration::from_secs(30));
    }

    /// The operator's `offline_check_count` governs both the threshold and
    /// the window: with 5 checks 10 s apart, four network failures are not
    /// yet an offline signal.
    #[test]
    fn from_scheduler_higher_count_widens_window() {
        let c = OfflineClassifier::from_scheduler(5, 10_000);
        assert_eq!(c.threshold, 5);
        assert_eq!(c.window(), Duration::from_secs(50));

        for _ in 0..4 {
            assert_eq!(
                c.classify_failure("s", &EngineKind::MesioFlv, &DownloadFailureKind::Network),
                None
            );
        }
        assert_eq!(
            c.classify_failure("s", &EngineKind::MesioFlv, &DownloadFailureKind::Network),
            Some(OfflineSignal::ConsecutiveFailures(5))
        );
    }

    /// One streamer reaching the threshold neither resets nor feeds another
    /// streamer's count.
    #[test]
    fn streamer_isolation_in_consecutive_counter() {
        let c = classic_classifier();
        let network = |streamer| {
            c.classify_failure(
                streamer,
                &EngineKind::MesioFlv,
                &DownloadFailureKind::Network,
            )
        };

        assert_eq!(network("a"), None);
        assert_eq!(network("b"), None);
        assert_eq!(network("a"), Some(OfflineSignal::ConsecutiveFailures(2)));
        assert_eq!(network("b"), Some(OfflineSignal::ConsecutiveFailures(2)));
    }
}
