//! Configuration for danmaku statistics aggregation.

use serde::{Deserialize, Serialize};

/// Tuning for [`super::StatisticsAggregator`].
///
/// The `*_capacity` fields are how many distinct keys are *tracked*; the
/// `top_*` fields are how many are *reported*. They are separate because the
/// tracking capacity governs accuracy — while fewer distinct keys than capacity
/// have been seen, Space-Saving never evicts and every count is exact — whereas
/// the reported size only affects payload size. Deriving one from the other
/// silently changes the accuracy regime when a caller asks for a longer list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatisticsConfig {
    /// Talkers and gifters returned by snapshots.
    pub top_talkers: usize,
    /// Words returned by snapshots.
    pub top_words: usize,
    /// Gift names returned by snapshots.
    pub top_gifts: usize,
    /// Distinct senders tracked before Space-Saving eviction begins.
    pub talker_capacity: usize,
    /// Distinct words tracked before Space-Saving eviction begins.
    pub word_capacity: usize,
    /// Distinct gift names tracked before eviction begins.
    pub gift_capacity: usize,
    /// Initial width of one rate-timeseries bucket.
    pub rate_bucket_secs: u64,
    /// Words to ignore on top of the built-in list, matched lowercased.
    pub extra_stop_words: Vec<String>,
}

impl Default for StatisticsConfig {
    fn default() -> Self {
        Self {
            top_talkers: 100,
            top_words: 50,
            top_gifts: 20,
            // Generous enough that ordinary rooms never evict, so their counts
            // are exact rather than Space-Saving approximations.
            talker_capacity: 2048,
            word_capacity: 2048,
            gift_capacity: 256,
            rate_bucket_secs: 10,
            extra_stop_words: Vec::new(),
        }
    }
}
