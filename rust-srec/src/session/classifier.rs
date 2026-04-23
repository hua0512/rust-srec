//! Per-engine classifier — turns a `Terminal::Failed` into an
//! [`OfflineSignal`] when the engine-side error is strong enough to be a
//! definitive offline signal that bypasses the slower hysteresis path.
//!
//! Rules (PR 2):
//!
//! - **mesio HLS / mesio FLV + `HttpClientError { status: 404 }`** → a
//!   playlist/manifest 404 that the engine surfaces is a definitive
//!   offline for the stream (the upstream platform has removed the
//!   resource). Returns [`OfflineSignal::PlaylistGone(404)`].
//! - **mesio HLS / mesio FLV + `Network` failures** — accumulate per
//!   streamer; two distinct failures inside a 60 s window are treated
//!   as a definitive offline. Returns
//!   [`OfflineSignal::ConsecutiveFailures(2)`]. Counter resets when a
//!   successful segment is observed (preserves Bilibili-style mid-stream
//!   RST reconnects).
//! - **ffmpeg / streamlink** — subprocess errors are too fuzzy to
//!   classify with high confidence. Returns `None`; the slower monitor
//!   path observes offline on the next successful status check.
//! - **Everything else** — `None`. Includes `SourceUnavailable`
//!   (the engine already gave up without a 404), `Processing` (writer
//!   error, independent of upstream state), `Io`, `OutputRootUnavailable`
//!   (infrastructure), `Configuration`, `RateLimited`, server 5xx, etc.
//!
//! The classifier is stateful (holds the per-streamer consecutive-
//! failure window). Constructed once per process and shared with
//! [`crate::session::SessionLifecycle`].

use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, info};

use crate::downloader::DownloadFailureKind;
use crate::session::state::OfflineSignal;

/// Time window for the consecutive-failures rule (plan §2b).
const CONSECUTIVE_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Number of failures inside [`CONSECUTIVE_FAILURE_WINDOW`] that trips
/// the definitive-offline signal.
const CONSECUTIVE_FAILURE_THRESHOLD: usize = 2;

/// A classifier inspects an engine failure and decides whether it is a
/// definitive offline signal worth bypassing the slower hysteresis path.
#[derive(Debug, Default)]
pub struct OfflineClassifier {
    /// Per-streamer timestamps of recent eligible (Network) failures.
    /// Pruned to the trailing [`CONSECUTIVE_FAILURE_WINDOW`] on each update.
    failure_log: DashMap<String, Vec<Instant>>,
}

impl OfflineClassifier {
    pub fn new() -> Self {
        Self::default()
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
            DownloadFailureKind::HttpClientError { status: 404 } => {
                info!(
                    streamer_id,
                    engine = ?engine_kind,
                    "OfflineClassifier: mesio 404 → DefinitiveOffline(PlaylistGone)"
                );
                Some(OfflineSignal::PlaylistGone(404))
            }
            DownloadFailureKind::Network => {
                self.record_network_failure_and_check(streamer_id)
            }
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
        let mut entry = self.failure_log.entry(streamer_id.to_string()).or_default();
        let log = entry.value_mut();

        // Prune anything outside the window before appending.
        log.retain(|t| now.duration_since(*t) < CONSECUTIVE_FAILURE_WINDOW);
        log.push(now);
        let count = log.len();

        if count >= CONSECUTIVE_FAILURE_THRESHOLD {
            // Clear after firing so the next N failures inside the next
            // window trip again (don't hold the counter at threshold).
            log.clear();
            info!(
                streamer_id,
                count,
                threshold = CONSECUTIVE_FAILURE_THRESHOLD,
                window_secs = CONSECUTIVE_FAILURE_WINDOW.as_secs(),
                "OfflineClassifier: consecutive Network failures → DefinitiveOffline(ConsecutiveFailures)"
            );
            Some(OfflineSignal::ConsecutiveFailures(
                CONSECUTIVE_FAILURE_THRESHOLD as u32,
            ))
        } else {
            debug!(
                streamer_id,
                count,
                threshold = CONSECUTIVE_FAILURE_THRESHOLD,
                window_secs = CONSECUTIVE_FAILURE_WINDOW.as_secs(),
                "OfflineClassifier: Network failure logged; below threshold"
            );
            None
        }
    }
}

/// Which download engine produced the failure. Distinguishing engines is
/// required because `mesio HLS` 404 (definitive) is not the same signal as
/// `ffmpeg` exit-code-1 (fuzzy).
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

    // ---- C1 — mesio HLS playlist 404 → PlaylistGone(404) -------------

    #[test]
    fn c1_mesio_hls_playlist_404_is_definitive_offline() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::HttpClientError { status: 404 },
        );
        assert_eq!(result, Some(OfflineSignal::PlaylistGone(404)));
    }

    #[test]
    fn c1_mesio_flv_404_is_definitive_offline() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioFlv,
            &DownloadFailureKind::HttpClientError { status: 404 },
        );
        assert_eq!(result, Some(OfflineSignal::PlaylistGone(404)));
    }

    // ---- C2 — mesio HLS network timeout alone → None -----------------

    #[test]
    fn c2_single_network_failure_does_not_classify() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "s1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::Network,
        );
        assert_eq!(result, None);
    }

    // ---- C3 — two consecutive Network failures within 60 s → Some ----

    #[test]
    fn c3_two_consecutive_network_failures_classify_as_definitive_offline() {
        let c = OfflineClassifier::new();

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
        let c = OfflineClassifier::new();

        // Manually seed the log with a timestamp 61 seconds in the past
        // so we don't have to block the test for a minute.
        let stale = Instant::now()
            .checked_sub(CONSECUTIVE_FAILURE_WINDOW + Duration::from_secs(1))
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
        let c = OfflineClassifier::new();

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
        let c = OfflineClassifier::new();
        // Even a 404 from an ffmpeg wrapper is not classified — ffmpeg
        // surfaces process-exit noise rather than clean HTTP statuses.
        let result = c.classify_failure(
            "s1",
            &EngineKind::Ffmpeg,
            &DownloadFailureKind::HttpClientError { status: 404 },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c6_ffmpeg_subprocess_error_is_none() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "s1",
            &EngineKind::Ffmpeg,
            &DownloadFailureKind::ProcessExit { code: Some(1) },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c7_streamlink_subprocess_error_is_none() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "s1",
            &EngineKind::Streamlink,
            &DownloadFailureKind::ProcessExit { code: Some(2) },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn c7_streamlink_network_never_accumulates() {
        let c = OfflineClassifier::new();
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

    /// Non-network, non-404 mesio failures return `None`.
    #[test]
    fn mesio_source_unavailable_returns_none() {
        let c = OfflineClassifier::new();
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
        let c = OfflineClassifier::new();
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
        let c = OfflineClassifier::new();

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
}
