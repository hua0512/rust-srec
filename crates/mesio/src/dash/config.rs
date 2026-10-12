//! Configuration of the DASH engine.
//!
//! Segment download, scheduling, reordering and byte budgets reuse the
//! segment-engine settings types shared with HLS; the DASH-specific parts are
//! manifest refresh, live start, Representation selection, and muxing of
//! separately delivered tracks.

use std::sync::Arc;
use std::time::Duration;

use crate::DownloaderConfig;
use crate::dash::manifest::{Clock, SystemClock};
use crate::error::DownloadError;
use crate::hls::config::{HlsEngineConfig, HlsFetcherConfig, HlsOutputConfig, HlsSchedulerConfig};

#[derive(Debug, Clone)]
pub struct DashConfig {
    /// HTTP client settings and parameters added to every MPD and segment
    /// request.
    pub base: DownloaderConfig,
    pub manifest: DashManifestConfig,
    pub live_start: LiveStartPolicy,
    pub selection: DashSelectionConfig,
    pub mux: DashMuxConfig,
    pub budget_split: DashBudgetSplit,
    /// Download concurrency, per selected track.
    pub scheduler: HlsSchedulerConfig,
    pub fetcher: HlsFetcherConfig,
    /// Reordering and gap policy per track. `max_pending_init_segments`
    /// must be 0 (unlimited) or at least 2: a track's current and next
    /// epoch inits can both arrive before the next epoch's media.
    pub output: HlsOutputConfig,
    /// Byte budgets shared by the selected tracks according to
    /// `budget_split`; identity policy and state bounds per track.
    pub engine: HlsEngineConfig,
    /// TTL of downloaded segments in the cache.
    pub processed_segment_ttl: Duration,
    /// Local clock; segment availability is computed from it plus the
    /// measured server offset. Replaceable for tests.
    pub clock: Arc<dyn Clock>,
}

impl Default for DashConfig {
    fn default() -> Self {
        Self {
            base: DownloaderConfig::default(),
            manifest: DashManifestConfig::default(),
            live_start: LiveStartPolicy::default(),
            selection: DashSelectionConfig::default(),
            mux: DashMuxConfig::default(),
            budget_split: DashBudgetSplit::default(),
            scheduler: HlsSchedulerConfig::default(),
            fetcher: HlsFetcherConfig::default(),
            output: HlsOutputConfig::default(),
            engine: HlsEngineConfig::default(),
            processed_segment_ttl: Duration::from_secs(60),
            clock: Arc::new(SystemClock),
        }
    }
}

impl DashConfig {
    /// Rejects settings the engine cannot run with.
    pub fn validate(&self) -> Result<(), DownloadError> {
        let invalid = |reason: &str| {
            Err(DownloadError::Configuration {
                reason: format!("DASH: {reason}"),
            })
        };
        if !self.selection.include_video && !self.selection.include_audio {
            return invalid("at least one of video and audio must be included");
        }
        if !(1..=99).contains(&self.budget_split.video_percent) {
            return invalid("budget_split.video_percent must be between 1 and 99");
        }
        if self.manifest.fetch_timeout.is_zero() {
            return invalid("manifest.fetch_timeout must be positive");
        }
        // Zero would stop the reactor from ever forwarding work, or the
        // muxer from ever reading audio, rather than mean "unlimited".
        if self.engine.max_pending_items == 0 {
            return invalid("engine.max_pending_items must be positive");
        }
        if self.engine.max_pending_payload_bytes == 0 {
            return invalid("engine.max_pending_payload_bytes must be positive");
        }
        if self.mux.max_buffered_bytes == 0 {
            return invalid("mux.max_buffered_bytes must be positive");
        }
        // A track's planner admits the current and the next epoch, and both
        // inits can arrive before the next epoch's media; retaining only one
        // evicts the first before its media is written, and the planner then
        // waits for media that waits for that init.
        if self.output.max_pending_init_segments == 1 {
            return invalid("output.max_pending_init_segments must be 0 (unlimited) or at least 2");
        }
        Ok(())
    }
}

/// MPD fetching and refreshing.
#[derive(Debug, Clone)]
pub struct DashManifestConfig {
    /// Timeout of one MPD, Segment Index or UTCTiming request.
    pub fetch_timeout: Duration,
    /// Lower bound of the refresh interval; the MPD's
    /// `minimumUpdatePeriod` applies when it is longer.
    pub min_refresh_interval: Duration,
    /// Consecutive failed refreshes (network, HTTP or invalid MPD) tolerated
    /// before the stream fails. Also the retries of a Segment Index request
    /// that failed with a network or server error.
    pub max_refresh_retries: u32,
    /// Delay after a failed refresh or Segment Index request, multiplied by
    /// the consecutive failure count.
    pub retry_delay: Duration,
    /// Correct the local clock with the server's: the MPD's `UTCTiming`
    /// sources, else the MPD response's `Date` header. Disable only when the
    /// local clock is known to be right (or injected, in tests).
    pub sync_clock: bool,
}

impl Default for DashManifestConfig {
    fn default() -> Self {
        Self {
            fetch_timeout: Duration::from_secs(15),
            min_refresh_interval: Duration::from_secs(1),
            max_refresh_retries: 5,
            retry_delay: Duration::from_secs(1),
            sync_clock: true,
        }
    }
}

/// Where recording of a dynamic presentation starts. Static presentations
/// always start at their first segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStartPolicy {
    /// The newest available segment.
    LiveEdge,
    /// The segment presented this far behind the live edge: the given delay,
    /// else the MPD's `suggestedPresentationDelay`, else three segment
    /// durations. Starting behind the edge keeps a few segments of slack
    /// against clock skew and late publishing.
    PresentationDelay(Option<Duration>),
}

impl Default for LiveStartPolicy {
    fn default() -> Self {
        Self::PresentationDelay(None)
    }
}

/// Which Representations are recorded.
#[derive(Debug, Clone)]
pub struct DashSelectionConfig {
    pub video: VideoSelection,
    pub audio: AudioSelection,
    /// Preferred audio language (RFC 5646 tag, e.g. `en` or `pt-BR`),
    /// matched against `AdaptationSet@lang` by primary subtag when no exact
    /// match exists. Any language is used when nothing matches.
    pub audio_language: Option<String>,
    pub include_video: bool,
    pub include_audio: bool,
}

impl Default for DashSelectionConfig {
    fn default() -> Self {
        Self {
            video: VideoSelection::default(),
            audio: AudioSelection::default(),
            audio_language: None,
            include_video: true,
            include_audio: true,
        }
    }
}

/// Video Representation choice within a Period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoSelection {
    /// Highest `@bandwidth`.
    #[default]
    Highest,
    /// Lowest `@bandwidth`.
    Lowest,
    /// Highest `@bandwidth` with `@height` at most this value; the smallest
    /// picture when none fits.
    MaxHeight(u32),
    /// `@bandwidth` closest to this value (bits per second).
    ClosestBandwidth(u64),
}

/// Audio Representation choice within the preferred language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioSelection {
    /// Highest `@bandwidth`.
    #[default]
    Highest,
    /// Lowest `@bandwidth`.
    Lowest,
    /// `@bandwidth` closest to this value (bits per second).
    ClosestBandwidth(u64),
}

/// Combining separately delivered video and audio into one stream.
#[derive(Debug, Clone)]
pub struct DashMuxConfig {
    /// How long a video segment waits, from its arrival at the muxer, for
    /// the audio it overlaps. After that the segment is written with whatever
    /// audio has arrived; audio arriving later for it is dropped. Also bounds
    /// the wait for the audio tail once the video has ended.
    pub max_audio_lag: Duration,
    /// Bytes buffered in the muxer across both tracks. Past this, a video
    /// segment waiting for audio is written without waiting further, and
    /// audio that runs ahead of video stops being read.
    pub max_buffered_bytes: usize,
}

impl Default for DashMuxConfig {
    fn default() -> Self {
        Self {
            max_audio_lag: Duration::from_secs(10),
            max_buffered_bytes: 64 * 1024 * 1024,
        }
    }
}

/// How the byte budgets of `DashConfig::engine` are divided between the
/// video and audio pipelines when both are recorded.
///
/// Each track gets a budget of its own: a budget shared by two independent
/// schedulers can be held entirely by one of them, leaving the other unable
/// to admit anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashBudgetSplit {
    /// Percentage for video (1–99); audio gets the rest.
    pub video_percent: u8,
}

impl Default for DashBudgetSplit {
    fn default() -> Self {
        Self { video_percent: 85 }
    }
}

impl DashBudgetSplit {
    /// The share of `total` for one track (`0` = unlimited stays unlimited).
    /// A limited budget never rounds down to 0, which would mean unlimited.
    pub(crate) fn share(self, total: u64, video: bool, both_tracks: bool) -> u64 {
        if total == 0 || !both_tracks {
            return total;
        }
        let percent = if video {
            u64::from(self.video_percent)
        } else {
            100 - u64::from(self.video_percent.min(100))
        };
        let share = u128::from(total) * u128::from(percent) / 100;
        u64::try_from(share).unwrap_or(total).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_split_divides_limited_budgets_only_when_both_tracks_run() {
        let split = DashBudgetSplit { video_percent: 85 };
        assert_eq!(split.share(1000, true, true), 850);
        assert_eq!(split.share(1000, false, true), 150);
        assert_eq!(split.share(1000, false, false), 1000);
        assert_eq!(split.share(0, true, true), 0);
        assert_eq!(split.share(1, false, true), 1);
    }

    #[test]
    fn validation_rejects_unusable_settings() {
        let mut config = DashConfig::default();
        assert!(config.validate().is_ok());
        config.output.max_pending_init_segments = 0;
        assert!(config.validate().is_ok(), "0 is unlimited");
        config.output.max_pending_init_segments = 2;
        assert!(config.validate().is_ok());
        config.selection.include_video = false;
        config.selection.include_audio = false;
        assert!(config.validate().is_err());

        let mut config = DashConfig::default();
        config.budget_split.video_percent = 100;
        assert!(config.validate().is_err());

        for zero in [
            |config: &mut DashConfig| config.engine.max_pending_items = 0,
            |config: &mut DashConfig| config.engine.max_pending_payload_bytes = 0,
            |config: &mut DashConfig| config.mux.max_buffered_bytes = 0,
            |config: &mut DashConfig| config.output.max_pending_init_segments = 1,
        ] {
            let mut config = DashConfig::default();
            zero(&mut config);
            assert!(matches!(
                config.validate(),
                Err(DownloadError::Configuration { .. })
            ));
        }
    }
}
