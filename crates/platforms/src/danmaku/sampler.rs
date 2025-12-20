//! Danmu sampling strategies.
//!
//! Provides different strategies for sampling danmu messages for statistics.

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for danmu sampling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DanmuSamplingConfig {
    /// Fixed interval sampling
    Fixed {
        /// Interval in seconds between samples
        interval_secs: u64,
    },
    /// Dynamic velocity-based sampling
    Velocity {
        /// Minimum interval in seconds
        min_interval_secs: u64,
        /// Maximum interval in seconds
        max_interval_secs: u64,
        /// Target number of danmus per sample period
        target_danmus_per_sample: u32,
    },
}

impl Default for DanmuSamplingConfig {
    fn default() -> Self {
        Self::Fixed { interval_secs: 10 }
    }
}

impl DanmuSamplingConfig {
    /// Create a fixed interval sampler config.
    pub fn fixed(interval_secs: u64) -> Self {
        Self::Fixed { interval_secs }
    }

    /// Create a velocity-based sampler config.
    pub fn velocity(
        min_interval_secs: u64,
        max_interval_secs: u64,
        target_danmus_per_sample: u32,
    ) -> Self {
        Self::Velocity {
            min_interval_secs,
            max_interval_secs,
            target_danmus_per_sample,
        }
    }
}

/// Trait for danmu sampling strategies.
pub trait DanmuSampler: Send + Sync {
    /// Record that a message was received.
    fn record_message(&mut self, timestamp: DateTime<Utc>);

    /// Check if we should take a sample now.
    fn should_sample(&self, now: DateTime<Utc>) -> bool;

    /// Mark that a sample was taken.
    fn mark_sampled(&mut self, timestamp: DateTime<Utc>);

    /// Get the current sampling interval.
    fn current_interval(&self) -> Duration;

    /// Reset the sampler state.
    fn reset(&mut self);
}

/// Fixed interval sampler.
///
/// Samples at a constant interval regardless of message velocity.
#[derive(Debug)]
pub struct FixedIntervalSampler {
    interval: Duration,
    last_sample: Option<DateTime<Utc>>,
}

impl FixedIntervalSampler {
    /// Create a new fixed interval sampler.
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval: Duration::from_secs(interval_secs),
            last_sample: None,
        }
    }
}

impl DanmuSampler for FixedIntervalSampler {
    fn record_message(&mut self, _timestamp: DateTime<Utc>) {
        // Fixed sampler doesn't adjust based on message rate
    }

    fn should_sample(&self, now: DateTime<Utc>) -> bool {
        match self.last_sample {
            None => true,
            Some(last) => {
                let elapsed = now.signed_duration_since(last);
                elapsed
                    >= chrono::Duration::from_std(self.interval).unwrap_or(chrono::Duration::MAX)
            }
        }
    }

    fn mark_sampled(&mut self, timestamp: DateTime<Utc>) {
        self.last_sample = Some(timestamp);
    }

    fn current_interval(&self) -> Duration {
        self.interval
    }

    fn reset(&mut self) {
        self.last_sample = None;
    }
}

/// Velocity-based sampler.
///
/// Adjusts sampling interval based on message velocity to maintain
/// a target number of messages per sample period.
#[derive(Debug)]
pub struct VelocitySampler {
    min_interval: Duration,
    max_interval: Duration,
    target_per_sample: u32,
    current_interval: Duration,
    last_sample: Option<DateTime<Utc>>,
    messages_since_sample: u32,
    /// Rolling window of message counts for velocity calculation
    velocity_window: Vec<(DateTime<Utc>, u32)>,
    /// Window duration for velocity calculation (default 60 seconds)
    window_duration: Duration,
}

impl VelocitySampler {
    /// Create a new velocity-based sampler.
    pub fn new(min_interval_secs: u64, max_interval_secs: u64, target_per_sample: u32) -> Self {
        let initial_interval = Duration::from_secs((min_interval_secs + max_interval_secs) / 2);
        Self {
            min_interval: Duration::from_secs(min_interval_secs),
            max_interval: Duration::from_secs(max_interval_secs),
            target_per_sample,
            current_interval: initial_interval,
            last_sample: None,
            messages_since_sample: 0,
            velocity_window: Vec::new(),
            window_duration: Duration::from_secs(60),
        }
    }

    /// Calculate current message velocity (messages per second).
    fn calculate_velocity(&self, now: DateTime<Utc>) -> f64 {
        let cutoff = now
            - chrono::Duration::from_std(self.window_duration)
                .unwrap_or(chrono::Duration::seconds(60));

        let total_messages: u32 = self
            .velocity_window
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(_, count)| count)
            .sum();

        let window_secs = self.window_duration.as_secs_f64();
        if window_secs > 0.0 {
            total_messages as f64 / window_secs
        } else {
            0.0
        }
    }

    /// Adjust the sampling interval based on current velocity.
    fn adjust_interval(&mut self, now: DateTime<Utc>) {
        let velocity = self.calculate_velocity(now);

        if velocity <= 0.0 {
            // No messages, use max interval
            self.current_interval = self.max_interval;
            return;
        }

        // Calculate ideal interval to get target_per_sample messages
        let ideal_interval_secs = self.target_per_sample as f64 / velocity;

        // Clamp to min/max bounds
        let clamped_secs = ideal_interval_secs
            .max(self.min_interval.as_secs_f64())
            .min(self.max_interval.as_secs_f64());

        self.current_interval = Duration::from_secs_f64(clamped_secs);
    }

    /// Prune old entries from the velocity window.
    fn prune_window(&mut self, now: DateTime<Utc>) {
        let cutoff = now
            - chrono::Duration::from_std(self.window_duration)
                .unwrap_or(chrono::Duration::seconds(60));
        self.velocity_window.retain(|(ts, _)| *ts >= cutoff);
    }
}

impl DanmuSampler for VelocitySampler {
    fn record_message(&mut self, timestamp: DateTime<Utc>) {
        self.messages_since_sample += 1;

        // Add to velocity window (aggregate by second)
        let second_start = timestamp.with_nanosecond(0).unwrap_or(timestamp);

        if let Some((last_ts, count)) = self.velocity_window.last_mut()
            && *last_ts == second_start
        {
            *count += 1;
            return;
        }

        self.velocity_window.push((second_start, 1));

        // Prune old entries periodically
        if self.velocity_window.len() > 120 {
            self.prune_window(timestamp);
        }
    }

    fn should_sample(&self, now: DateTime<Utc>) -> bool {
        match self.last_sample {
            None => true,
            Some(last) => {
                let elapsed = now.signed_duration_since(last);
                elapsed
                    >= chrono::Duration::from_std(self.current_interval)
                        .unwrap_or(chrono::Duration::MAX)
            }
        }
    }

    fn mark_sampled(&mut self, timestamp: DateTime<Utc>) {
        self.last_sample = Some(timestamp);
        self.messages_since_sample = 0;
        self.adjust_interval(timestamp);
    }

    fn current_interval(&self) -> Duration {
        self.current_interval
    }

    fn reset(&mut self) {
        self.last_sample = None;
        self.messages_since_sample = 0;
        self.velocity_window.clear();
        self.current_interval =
            Duration::from_secs((self.min_interval.as_secs() + self.max_interval.as_secs()) / 2);
    }
}

/// Create a sampler from configuration.
pub fn create_sampler(config: &DanmuSamplingConfig) -> Box<dyn DanmuSampler> {
    match config {
        DanmuSamplingConfig::Fixed { interval_secs } => {
            Box::new(FixedIntervalSampler::new(*interval_secs))
        }
        DanmuSamplingConfig::Velocity {
            min_interval_secs,
            max_interval_secs,
            target_danmus_per_sample,
        } => Box::new(VelocitySampler::new(
            *min_interval_secs,
            *max_interval_secs,
            *target_danmus_per_sample,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_fixed_sampler_initial_sample() {
        let sampler = FixedIntervalSampler::new(10);
        let now = Utc::now();

        // Should sample immediately on first check
        assert!(sampler.should_sample(now));
    }

    #[test]
    fn test_fixed_sampler_interval() {
        let mut sampler = FixedIntervalSampler::new(10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();

        // First sample
        assert!(sampler.should_sample(base));
        sampler.mark_sampled(base);

        // 5 seconds later - should not sample
        let t1 = base + chrono::Duration::seconds(5);
        assert!(!sampler.should_sample(t1));

        // 10 seconds later - should sample
        let t2 = base + chrono::Duration::seconds(10);
        assert!(sampler.should_sample(t2));
    }

    #[test]
    fn test_velocity_sampler_adjusts_interval() {
        let mut sampler = VelocitySampler::new(5, 30, 10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();

        // Simulate high velocity (many messages)
        for i in 0..100 {
            let ts = base + chrono::Duration::milliseconds(i * 100);
            sampler.record_message(ts);
        }

        // Take a sample
        let sample_time = base + chrono::Duration::seconds(10);
        sampler.mark_sampled(sample_time);

        // With high velocity, interval should be closer to minimum
        let interval = sampler.current_interval();
        assert!(
            interval <= Duration::from_secs(15),
            "Interval should decrease with high velocity"
        );
    }

    #[test]
    fn test_velocity_sampler_low_velocity() {
        let mut sampler = VelocitySampler::new(5, 30, 10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();

        // Simulate low velocity (few messages)
        sampler.record_message(base);
        sampler.record_message(base + chrono::Duration::seconds(30));

        // Take a sample
        let sample_time = base + chrono::Duration::seconds(60);
        sampler.mark_sampled(sample_time);

        // With low velocity, interval should be at maximum
        let interval = sampler.current_interval();
        assert_eq!(
            interval,
            Duration::from_secs(30),
            "Interval should be max with low velocity"
        );
    }

    #[test]
    fn test_create_sampler_fixed() {
        let config = DanmuSamplingConfig::fixed(15);
        let sampler = create_sampler(&config);

        assert_eq!(sampler.current_interval(), Duration::from_secs(15));
    }

    #[test]
    fn test_create_sampler_velocity() {
        let config = DanmuSamplingConfig::velocity(5, 30, 10);
        let sampler = create_sampler(&config);

        // Initial interval should be midpoint
        let interval = sampler.current_interval();
        assert!(interval >= Duration::from_secs(5));
        assert!(interval <= Duration::from_secs(30));
    }

    #[test]
    fn test_sampler_reset() {
        let mut sampler = FixedIntervalSampler::new(10);
        let now = Utc::now();

        sampler.mark_sampled(now);
        assert!(!sampler.should_sample(now));

        sampler.reset();
        assert!(sampler.should_sample(now));
    }
}
