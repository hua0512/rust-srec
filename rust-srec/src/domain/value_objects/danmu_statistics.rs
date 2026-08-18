//! User-facing settings for danmu statistics aggregation.

use platforms_parser::danmaku::StatisticsConfig;
use serde::{Deserialize, Serialize};

/// How a session's danmu statistics are aggregated.
///
/// Every field has a default, and `#[serde(default)]` applies per field, so a
/// partial override such as `{"top_talkers": 200}` is valid and inherits the rest.
///
/// The `*_capacity` fields are how many distinct keys are *tracked*; the `top_*`
/// fields are how many are *reported*. They are deliberately separate: capacity
/// governs accuracy, because while fewer distinct keys than capacity have been
/// seen the underlying Space-Saving structures never evict and every count is
/// exact, whereas the reported size only affects payload size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DanmuStatisticsConfig {
    /// Whether to aggregate statistics at all.
    ///
    /// Turning this off still records the danmu XML files; it only stops the
    /// per-session aggregates — which include sender IDs and display names —
    /// from being computed and stored. A session recorded with statistics off has
    /// no statistics row, so the API reports them as unavailable.
    pub enabled: bool,
    /// Talkers and gifters reported per session.
    pub top_talkers: usize,
    /// Frequent words reported per session.
    pub top_words: usize,
    /// Gift names reported per session.
    pub top_gifts: usize,
    /// Distinct senders tracked before approximation begins.
    pub talker_capacity: usize,
    /// Distinct words tracked before approximation begins.
    pub word_capacity: usize,
    /// Distinct gift names tracked before approximation begins.
    pub gift_capacity: usize,
    /// Width of one activity-timeline bucket, in seconds. The aggregator doubles
    /// this for sessions long enough to exceed its point budget.
    pub rate_bucket_secs: u64,
    /// Additional words excluded from frequency counts, matched lowercased,
    /// alongside the built-in list.
    pub extra_stop_words: Vec<String>,
}

impl Default for DanmuStatisticsConfig {
    fn default() -> Self {
        let aggregator = StatisticsConfig::default();
        Self {
            enabled: true,
            top_talkers: aggregator.top_talkers,
            top_words: aggregator.top_words,
            top_gifts: aggregator.top_gifts,
            talker_capacity: aggregator.talker_capacity,
            word_capacity: aggregator.word_capacity,
            gift_capacity: aggregator.gift_capacity,
            rate_bucket_secs: aggregator.rate_bucket_secs,
            extra_stop_words: Vec::new(),
        }
    }
}

impl DanmuStatisticsConfig {
    /// Upper bound on tracking capacity.
    ///
    /// Eviction is a linear scan of the counter map, so capacity trades accuracy
    /// against per-message cost: measured throughput falls from ~390k to ~80k
    /// messages per second between capacity 256 and 4000 in a high-churn room.
    /// Both are far above any real room's rate, and this ceiling keeps it that
    /// way even if a user maximises every field.
    const MAX_CAPACITY: usize = 8192;
    const MIN_CAPACITY: usize = 64;
    /// Upper bound on reported list sizes. Beyond this the payload dominates the
    /// response and no UI presents the entries usefully.
    const MAX_REPORTED: usize = 500;
    /// A bucket wider than an hour makes the activity timeline meaningless.
    const MAX_BUCKET_SECS: u64 = 3600;
    /// Enough for a curated block list without letting one streamer's config
    /// dominate the per-message word filter.
    const MAX_STOP_WORDS: usize = 500;

    /// Clamp user-supplied values into workable ranges.
    ///
    /// Clamps rather than rejects, matching how the other numeric config fields
    /// behave, so an out-of-range value degrades to the nearest usable one
    /// instead of failing a recording.
    pub fn sanitized(mut self) -> Self {
        self.top_talkers = self.top_talkers.clamp(1, Self::MAX_REPORTED);
        self.top_words = self.top_words.clamp(1, Self::MAX_REPORTED);
        self.top_gifts = self.top_gifts.clamp(1, Self::MAX_REPORTED);
        self.talker_capacity = self
            .talker_capacity
            .clamp(Self::MIN_CAPACITY, Self::MAX_CAPACITY);
        self.word_capacity = self
            .word_capacity
            .clamp(Self::MIN_CAPACITY, Self::MAX_CAPACITY);
        self.gift_capacity = self.gift_capacity.clamp(16, Self::MAX_CAPACITY);
        self.rate_bucket_secs = self.rate_bucket_secs.clamp(1, Self::MAX_BUCKET_SECS);

        // Reported lists cannot exceed what is tracked, or the tail would be
        // padded with nothing.
        self.top_talkers = self.top_talkers.min(self.talker_capacity);
        self.top_words = self.top_words.min(self.word_capacity);
        self.top_gifts = self.top_gifts.min(self.gift_capacity);

        self.extra_stop_words.truncate(Self::MAX_STOP_WORDS);
        for word in &mut self.extra_stop_words {
            *word = word.trim().to_lowercase();
        }
        self.extra_stop_words.retain(|word| !word.is_empty());

        self
    }

    /// The aggregator's view of these settings.
    pub fn to_aggregator_config(&self) -> StatisticsConfig {
        StatisticsConfig {
            top_talkers: self.top_talkers,
            top_words: self.top_words,
            top_gifts: self.top_gifts,
            talker_capacity: self.talker_capacity,
            word_capacity: self.word_capacity,
            gift_capacity: self.gift_capacity,
            rate_bucket_secs: self.rate_bucket_secs,
            extra_stop_words: self.extra_stop_words.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_inherits_defaults() {
        let config: DanmuStatisticsConfig =
            serde_json::from_str(r#"{"top_talkers": 200}"#).expect("partial config parses");

        assert_eq!(config.top_talkers, 200);
        assert_eq!(
            config.top_words,
            DanmuStatisticsConfig::default().top_words,
            "unspecified fields keep their defaults"
        );
        assert!(config.enabled);
    }

    #[test]
    fn sanitized_clamps_out_of_range_values() {
        let config = DanmuStatisticsConfig {
            top_talkers: 100_000,
            top_words: 0,
            talker_capacity: 1,
            rate_bucket_secs: 0,
            ..Default::default()
        }
        .sanitized();

        assert_eq!(config.top_talkers, DanmuStatisticsConfig::MIN_CAPACITY);
        assert_eq!(config.top_words, 1);
        assert_eq!(
            config.talker_capacity,
            DanmuStatisticsConfig::MIN_CAPACITY,
            "capacity has a floor so tracking stays useful"
        );
        assert_eq!(config.rate_bucket_secs, 1);
    }

    /// A reported list longer than what is tracked would have nothing to fill
    /// its tail with.
    #[test]
    fn sanitized_caps_reported_lists_at_capacity() {
        let config = DanmuStatisticsConfig {
            top_talkers: 500,
            talker_capacity: 64,
            ..Default::default()
        }
        .sanitized();

        assert_eq!(config.top_talkers, 64);
    }

    #[test]
    fn sanitized_normalizes_stop_words() {
        let config = DanmuStatisticsConfig {
            extra_stop_words: vec!["  LOL ".to_string(), String::new(), "Nice".to_string()],
            ..Default::default()
        }
        .sanitized();

        assert_eq!(config.extra_stop_words, vec!["lol", "nice"]);
    }
}
