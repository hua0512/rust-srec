//! Danmu sampling configuration value object.

use serde::{Deserialize, Serialize};

/// Danmu sampling strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingStrategy {
    /// Fixed interval sampling.
    #[default]
    Fixed,
    /// Dynamic velocity-based sampling.
    Dynamic,
}

/// Danmu sampling configuration.
///
/// Controls how danmu messages are sampled for statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DanmuSamplingConfig {
    /// Sampling strategy to use.
    #[serde(default)]
    pub strategy: SamplingStrategy,

    /// Interval in seconds for fixed sampling (default: 10).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u32,

    /// Minimum interval in seconds for dynamic sampling.
    #[serde(default = "default_min_interval_secs")]
    pub min_interval_secs: u32,

    /// Maximum interval in seconds for dynamic sampling.
    #[serde(default = "default_max_interval_secs")]
    pub max_interval_secs: u32,

    /// Target number of danmus per sample for dynamic sampling.
    #[serde(default = "default_target_danmus")]
    pub target_danmus_per_sample: u32,
}

fn default_interval_secs() -> u32 {
    10
}

fn default_min_interval_secs() -> u32 {
    5
}

fn default_max_interval_secs() -> u32 {
    60
}

fn default_target_danmus() -> u32 {
    100
}

impl DanmuSamplingConfig {
    /// Create a fixed interval sampling config.
    pub fn fixed(interval_secs: u32) -> Self {
        Self {
            strategy: SamplingStrategy::Fixed,
            interval_secs,
            ..Default::default()
        }
    }

    /// Create a dynamic sampling config.
    pub fn dynamic(min_interval: u32, max_interval: u32, target_danmus: u32) -> Self {
        Self {
            strategy: SamplingStrategy::Dynamic,
            interval_secs: default_interval_secs(),
            min_interval_secs: min_interval,
            max_interval_secs: max_interval,
            target_danmus_per_sample: target_danmus,
        }
    }

    /// Get the current sampling interval based on danmu rate.
    ///
    /// For fixed strategy, always returns the fixed interval.
    /// For dynamic strategy, calculates based on recent danmu rate.
    pub fn calculate_interval(&self, recent_danmu_rate: f64) -> u32 {
        match self.strategy {
            SamplingStrategy::Fixed => self.interval_secs,
            SamplingStrategy::Dynamic => {
                if recent_danmu_rate <= 0.0 {
                    return self.max_interval_secs;
                }

                // Calculate interval to achieve target danmus per sample
                let ideal_interval =
                    (self.target_danmus_per_sample as f64 / recent_danmu_rate) as u32;

                // Clamp to min/max bounds
                ideal_interval.clamp(self.min_interval_secs, self.max_interval_secs)
            }
        }
    }
}

impl Default for DanmuSamplingConfig {
    fn default() -> Self {
        Self {
            strategy: SamplingStrategy::Fixed,
            interval_secs: default_interval_secs(),
            min_interval_secs: default_min_interval_secs(),
            max_interval_secs: default_max_interval_secs(),
            target_danmus_per_sample: default_target_danmus(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DanmuSamplingConfig::default();
        assert_eq!(config.strategy, SamplingStrategy::Fixed);
        assert_eq!(config.interval_secs, 10);
    }

    #[test]
    fn test_fixed_sampling() {
        let config = DanmuSamplingConfig::fixed(30);
        assert_eq!(config.strategy, SamplingStrategy::Fixed);
        assert_eq!(config.interval_secs, 30);

        // Fixed always returns the same interval
        assert_eq!(config.calculate_interval(0.0), 30);
        assert_eq!(config.calculate_interval(100.0), 30);
    }

    #[test]
    fn test_dynamic_sampling() {
        let config = DanmuSamplingConfig::dynamic(5, 60, 100);
        assert_eq!(config.strategy, SamplingStrategy::Dynamic);

        // High rate = shorter interval
        assert_eq!(config.calculate_interval(20.0), 5); // 100/20 = 5

        // Low rate = longer interval
        assert_eq!(config.calculate_interval(2.0), 50); // 100/2 = 50

        // Very low rate = max interval
        assert_eq!(config.calculate_interval(0.5), 60); // 100/0.5 = 200, clamped to 60

        // Zero rate = max interval
        assert_eq!(config.calculate_interval(0.0), 60);
    }

    #[test]
    fn test_serialization() {
        let config = DanmuSamplingConfig::fixed(15);
        let json = serde_json::to_string(&config).unwrap();
        let parsed: DanmuSamplingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.interval_secs, 15);
    }
}
