//! Adaptive manifest refresh pacing.

use std::time::Duration;

/// Tracks segment arrival patterns to adaptively adjust manifest refresh
/// intervals: aggressive when behind the live edge, backed off when refreshes
/// keep coming back empty.
pub(crate) struct AdaptiveRefreshTracker {
    enabled: bool,
    min_interval: Duration,
    max_interval: Duration,
    /// Recent refresh results: true = got new segments.
    recent_results: std::collections::VecDeque<bool>,
    consecutive_empty: u32,
    /// New segments discovered on the most recent refresh; >1 usually means
    /// we are behind and should poll harder.
    last_new_segments_count: usize,
}

impl AdaptiveRefreshTracker {
    pub(crate) fn new(enabled: bool, min_interval: Duration, max_interval: Duration) -> Self {
        Self {
            enabled,
            min_interval,
            max_interval,
            recent_results: std::collections::VecDeque::with_capacity(10),
            consecutive_empty: 0,
            last_new_segments_count: 0,
        }
    }

    pub(crate) fn record_refresh(&mut self, new_segments_count: usize) {
        self.last_new_segments_count = new_segments_count;
        let got_segments = new_segments_count > 0;

        if self.recent_results.len() >= 10 {
            self.recent_results.pop_front();
        }
        self.recent_results.push_back(got_segments);

        if got_segments {
            self.consecutive_empty = 0;
        } else {
            self.consecutive_empty += 1;
        }
    }

    fn clamp_interval(&self, interval: Duration) -> Duration {
        interval.max(self.min_interval).min(self.max_interval)
    }

    pub(crate) fn get_refresh_interval(&self, default_interval: Duration) -> Duration {
        if !self.enabled {
            return default_interval;
        }

        let mut interval = default_interval;

        if self.last_new_segments_count >= 2 {
            // Multiple unseen segments: we are behind, catch up.
            interval = self.min_interval;
        } else if self.consecutive_empty >= 3 {
            // Exponential backoff after several empty refreshes.
            let backoff_factor = 1.5_f64.powi(self.consecutive_empty.min(5) as i32);
            interval = Duration::from_secs_f64(default_interval.as_secs_f64() * backoff_factor);
        } else {
            let recent_success_rate = self.recent_results.iter().filter(|&&got| got).count() as f64
                / self.recent_results.len().max(1) as f64;
            if recent_success_rate > 0.8 && self.recent_results.len() >= 5 {
                interval = Duration::from_secs_f64(default_interval.as_secs_f64() * 0.8);
            }
        }

        self.clamp_interval(interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_refresh_backoff_respects_min_interval() {
        let mut tracker =
            AdaptiveRefreshTracker::new(true, Duration::from_millis(500), Duration::from_secs(3));
        for _ in 0..3 {
            tracker.record_refresh(0);
        }
        let interval = tracker.get_refresh_interval(Duration::from_millis(100));
        assert!(interval >= Duration::from_millis(500));

        for _ in 0..7 {
            tracker.record_refresh(0);
        }
        let interval = tracker.get_refresh_interval(Duration::from_secs(10));
        assert!(interval <= Duration::from_secs(3));
    }

    #[test]
    fn adaptive_refresh_catches_up_when_behind() {
        let mut tracker =
            AdaptiveRefreshTracker::new(true, Duration::from_millis(500), Duration::from_secs(3));
        tracker.record_refresh(3);
        let interval = tracker.get_refresh_interval(Duration::from_secs(1));
        assert_eq!(interval, Duration::from_millis(500));
    }
}
