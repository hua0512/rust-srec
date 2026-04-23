//! Per-engine classifier — turns a `Terminal::Failed` into an
//! `OfflineSignal` when the engine-side error is strong enough to be a
//! definitive offline signal.
//!
//! **PR 1 ships a stub** — every classification returns `None`. PR 2 fills in:
//!
//! - mesio HLS `HttpClientError { status: 404 }` (playlist) → `Some(PlaylistGone(404))`.
//! - mesio HLS / mesio FLV `Network` (timeout / stall) accumulating to N=2
//!   within a 60 s window → `Some(ConsecutiveFailures(2))`. Counter resets
//!   on any successful segment download.
//! - ffmpeg / streamlink subprocess errors → `None` (subprocess errors are
//!   too fuzzy to map confidently).
//!
//! Keeping PR 1 strictly behaviour-preserving on the offline-detection path
//! lets the SessionLifecycle migration land independently of the classifier
//! work; PR 2 builds on top.

use crate::downloader::DownloadFailureKind;
use crate::session::state::OfflineSignal;

/// A classifier inspects an engine failure and decides whether it is a
/// definitive offline signal worth bypassing the slower hysteresis path.
///
/// Stateful (to support the consecutive-failures window). Constructed once
/// per process and shared with `SessionLifecycle`.
#[derive(Debug, Default)]
pub struct OfflineClassifier {
    // PR 2 will add per-streamer counters / windows here.
    _placeholder: (),
}

impl OfflineClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify a terminal failure against the streamer's recent history.
    ///
    /// Returns `Some(signal)` only when the failure is a high-confidence
    /// offline signal that should bypass the slower hysteresis path.
    /// PR 1 always returns `None`; PR 2 implements the real classification.
    pub fn classify_failure(
        &self,
        _streamer_id: &str,
        _engine_kind: &EngineKind,
        _failure: &DownloadFailureKind,
    ) -> Option<OfflineSignal> {
        None
    }

    /// Reset the per-streamer consecutive-failures counter when a successful
    /// segment is observed (preserves Bilibili-style mid-stream RST reconnects).
    pub fn note_successful_segment(&self, _streamer_id: &str) {
        // No-op in PR 1.
    }
}

/// Which download engine produced the failure. Distinguishing engines is
/// required because `mesio HLS` 404 (definitive) is not the same signal as
/// `ffmpeg` exit-code-1 (fuzzy).
///
/// Mirrors the variants of `crate::downloader::engine::EngineType` but is
/// owned here to avoid a circular dep on the engine module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    MesioHls,
    MesioFlv,
    Ffmpeg,
    Streamlink,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::DownloadFailureKind;

    #[test]
    fn pr1_classifier_returns_none_for_hls_404() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "streamer-1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::HttpClientError { status: 404 },
        );
        assert_eq!(result, None, "PR 1 stub must not return a signal yet");
    }

    #[test]
    fn pr1_classifier_returns_none_for_network_timeout() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "streamer-1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::Network,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn pr1_classifier_returns_none_for_source_unavailable() {
        let c = OfflineClassifier::new();
        let result = c.classify_failure(
            "streamer-1",
            &EngineKind::MesioHls,
            &DownloadFailureKind::SourceUnavailable,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn pr1_classifier_returns_none_for_ffmpeg_subprocess_error() {
        let c = OfflineClassifier::new();
        // PR 1 stub: subprocess errors classified as None (this is also
        // the long-term policy — see the plan's "out of scope" section).
        let result = c.classify_failure(
            "streamer-1",
            &EngineKind::Ffmpeg,
            &DownloadFailureKind::Other,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn note_successful_segment_is_noop_in_pr1() {
        let c = OfflineClassifier::new();
        // Just confirm it doesn't panic.
        c.note_successful_segment("streamer-1");
    }
}
